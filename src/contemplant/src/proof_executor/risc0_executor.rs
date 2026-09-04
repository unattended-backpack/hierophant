use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Result, anyhow};
use log::{error, info};
use network_lib::{
    ContemplantProofStatus, ProgressUpdate, Risc0ProofMode, Risc0ProofRequest, VmKind,
};
use risc0_zkvm::{ExecutorEnv, ProverOpts, Receipt, default_executor, default_prover};
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use tokio::{sync::mpsc, time::Instant};

#[derive(Clone)]
pub struct Risc0Executor {
    // CPU vs CUDA. Informational: risc0-zkvm 3.x selects the prover at
    // compile time via the `cuda` feature, not at runtime. Drives capability
    // advertising to the worker registry.
    pub backend: ProverBackend,
    // True when the operator has opted into Groth16 (needs the vendored
    // prover assets + docker shim; see Dockerfile.contemplant).
    pub groth16_enabled: bool,
    // Whether to run the quick execute pass that yields the segment count for
    // the cycle-rate ETA (config `compute_proof_totals`, default true). When
    // false, proofs report `Estimating` with no ETA.
    pub compute_totals: bool,
}

impl Risc0Executor {
    pub fn new(backend: ProverBackend, groth16_enabled: bool, compute_totals: bool) -> Self {
        Self {
            backend,
            groth16_enabled,
            compute_totals,
        }
    }
}

// Progress is the cycle-rate ETA model (see rate_model): a quick execute pass
// yields RISC Zero's segment count (its size signal), from which this
// contemplant's learned per-(vm,mode) rate produces a live ETA. This replaces
// the old scoped `tracing` subscriber that counted `prove_segment` events —
// fragile (marker could move/compile out) and prone to >100% overshoot when
// the recursion passes emitted more segments than the pre-count.
pub(super) async fn execute(
    state: WorkerState,
    executor: Risc0Executor,
    proof_request: Risc0ProofRequest,
    // Retained for genuinely fatal, environment-level errors; per-proof
    // failures report `Unexecutable` and keep the worker alive.
    _exit_sender: mpsc::Sender<String>,
) {
    info!(
        "Received RISC Zero proof request {} (mode {}, backend {:?})",
        proof_request.request_id,
        proof_request.mode.as_str(),
        executor.backend,
    );

    let initial_status = ContemplantProofStatus::unexecuted();
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    let request_id = proof_request.request_id;

    // Executing: no size / ETA yet.
    state
        .proof_store_client
        .proof_progress_update(request_id, ProgressUpdate::executing())
        .await;

    let display = format!(
        "RISC Zero {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );
    let groth16_enabled = executor.groth16_enabled;
    let compute_totals = executor.compute_totals;

    tokio::task::spawn(async move {
        // Total contemplant-side wall-clock; ETA + recorded sample cover the
        // execute pass + prove.
        let start_time = Instant::now();

        let elf = proof_request.elf;
        let input = proof_request.input;
        let mode = proof_request.mode;
        let mode_name = mode.as_str().to_string();
        let wrap_of = proof_request.wrap_of;

        // Size + ETA. Fresh proofs get a segment count from a quick execute
        // pass (no proving); a wrap has no cycle signal (it compresses an
        // existing receipt), so it gets no ETA. risc0-zkvm calls are
        // CPU-blocking, so the pass runs on a blocking thread.
        let mut size: Option<u64> = None;
        if compute_totals && wrap_of.is_none() {
            let elf_c = elf.clone();
            let input_c = input.clone();
            match tokio::task::spawn_blocking(move || -> Result<u64> {
                let env = ExecutorEnv::builder()
                    .write_slice(&input_c)
                    .build()
                    .map_err(|e| anyhow!("Build RISC Zero ExecutorEnv (size): {e}"))?;
                let session = default_executor()
                    .execute(env, &elf_c)
                    .map_err(|e| anyhow!("RISC Zero execute (size): {e}"))?;
                Ok(session.segments.len() as u64)
            })
            .await
            {
                Ok(Ok(n)) => {
                    size = Some(n);
                    let update = match state
                        .rate_model
                        .lock()
                        .await
                        .estimate_secs(VmKind::Risc0, &mode_name, n)
                    {
                        Some(est) => ProgressUpdate::proving(n, est),
                        None => ProgressUpdate::no_history(),
                    };
                    state
                        .proof_store_client
                        .proof_progress_update(request_id, update)
                        .await;
                }
                Ok(Err(e)) => error!("RISC Zero size pass for {display} failed (no ETA): {e}"),
                Err(e) => error!("RISC Zero size pass join error for {display}: {e}"),
            }
        }

        // Prove (CPU-blocking) — byte-identical to before, just without the
        // scoped tracing subscriber.
        let proof_res: Result<Vec<u8>> = tokio::task::spawn_blocking(move || {
            let opts = match mode {
                Risc0ProofMode::Composite => ProverOpts::composite(),
                Risc0ProofMode::Succinct => ProverOpts::succinct(),
                Risc0ProofMode::Groth16 => {
                    if !groth16_enabled {
                        return Err(anyhow!(
                            "Groth16 proofs are not enabled on this contemplant. Set `groth16_enabled = true` on the risc0 [[provers]] entry (or CONTEMPLANT_RISC0_GROTH16=true); requires the vendored prover assets + docker shim provided by Dockerfile.contemplant."
                        ));
                    }
                    ProverOpts::groth16()
                }
            };

            let prover = default_prover();
            let wrapped_receipt: Receipt = if let Some(source_bytes) = wrap_of {
                // Two-step wrap: compress an existing STARK receipt (almost
                // always to Groth16 for onchain verification).
                let source: Receipt = bincode::deserialize(&source_bytes)
                    .map_err(|e| anyhow!("Deserialize source receipt for wrap: {e}"))?;
                prover
                    .compress(&opts, &source)
                    .map_err(|e| anyhow!("RISC Zero compress (wrap) error: {e}"))?
            } else {
                let env = ExecutorEnv::builder()
                    .write_slice(&input)
                    .build()
                    .map_err(|e| anyhow!("Build RISC Zero ExecutorEnv: {e}"))?;
                prover
                    .prove_with_opts(env, &elf, &opts)
                    .map_err(|e| anyhow!("RISC Zero prover error: {e}"))?
                    .receipt
            };

            bincode::serialize(&wrapped_receipt)
                .map_err(|e| anyhow!("Serialize RISC Zero receipt: {e}"))
        })
        .await
        .map_err(|e| anyhow!("RISC Zero prover join error: {e}"))
        .and_then(|inner| inner);

        let elapsed_secs = start_time.elapsed().as_secs_f64();
        let minutes = (elapsed_secs / 60.0).round() as u32;

        match proof_res {
            Ok(receipt_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                // Learn this box's throughput for this (vm, mode).
                if let Some(n) = size {
                    state
                        .rate_model
                        .lock()
                        .await
                        .record(VmKind::Risc0, &mode_name, n, elapsed_secs);
                }
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(receipt_bytes),
                    )
                    .await;
                state
                    .proof_store_client
                    .proof_progress_update(request_id, ProgressUpdate::Done)
                    .await;
            }
            Err(e) => {
                let error_msg = format!("Error proving {display} at minute {minutes}: {e}");
                error!("{error_msg}");
                state
                    .proof_store_client
                    .proof_status_update(request_id, ExecutionStatus::Unexecutable.into(), None)
                    .await;
            }
        }
    });
}
