use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Result, anyhow};
use log::{error, info};
use network_lib::{
    ContemplantProofStatus, ProgressUpdate, ProvePhase, Risc0ProofMode, Risc0ProofRequest,
};
use risc0_zkvm::{Executor, ExecutorEnv, ProverOpts, Receipt, default_executor, default_prover};
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::{sync::mpsc, time::Instant};
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

// RISC0 wraps each proven segment in a `tracing::debug!("prove_segment")`
// event (its ProverServer::prove_segment default; see risc0-zkvm
// host/server/prove/mod.rs). We attach a scoped tracing subscriber with
// this counting layer around the (otherwise untouched) `prove_with_opts`
// call so the proving path is byte-identical and we merely observe its
// own instrumentation. Best-effort: if the marker ever moves or is
// compiled out, R0 progress degrades to the coarse phase markers the
// executor emits directly, and proving is entirely unaffected. `total`
// is 0 (indeterminate): a live per-segment count without a percentage.
struct R0SegmentLayer {
    count: Arc<AtomicU64>,
    // Exact segment total from the pre-proving execute pass (0 = unknown,
    // reported as indeterminate).
    total: Arc<AtomicU64>,
    tx: mpsc::UnboundedSender<ProgressUpdate>,
}

#[derive(Default)]
struct ProveSegmentDetector {
    hit: bool,
}
impl Visit for ProveSegmentDetector {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && format!("{value:?}").contains("prove_segment") {
            self.hit = true;
        }
    }
}

impl R0SegmentLayer {
    fn tick(&self) {
        let done = self.count.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.tx.send(ProgressUpdate::Phase {
            phase: ProvePhase::Prove,
            done,
            total: self.total.load(Ordering::Relaxed),
        });
    }
}

impl<S> Layer<S> for R0SegmentLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        // The event's own name is the message for `debug!("prove_segment")`.
        if event.metadata().name().contains("prove_segment") {
            self.tick();
            return;
        }
        let mut d = ProveSegmentDetector::default();
        event.record(&mut d);
        if d.hit {
            self.tick();
        }
    }
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if attrs.metadata().name() == "prove_segment" {
            self.tick();
        }
    }
}

#[derive(Clone)]
pub struct Risc0Executor {
    // CPU vs CUDA. When CUDA, the process must have been built with the
    // `enable-risc0-cuda` cargo feature and launched with GPU access at
    // runtime (nvidia-container-runtime / --gpus all). We set the
    // RISC0_PROVER env var at executor construction so risc0-zkvm's
    // default_prover() picks the right backend; the env var is process-wide
    // but our contemplant only has one risc0 executor at a time, so there's
    // no conflict.
    pub backend: ProverBackend,
    // True when the operator has opted into Groth16. This requires the
    // vendored prover assets under /opt/risc0-groth16-prover/ and the
    // /usr/local/bin/docker shim that intercepts risc0-groth16's
    // `docker run risczero/risc0-groth16-prover:<tag>` invocation and
    // dispatches to those assets. See Dockerfile.contemplant +
    // container/risc0-groth16-shim/docker. On CUDA builds the CPU shim is
    // bypassed entirely; risc0-groth16 has its own in-process cuda path.
    pub groth16_enabled: bool,
    // Run an execute pass before proving to learn the exact segment total
    // so progress is a percentage rather than a bare count.
    pub compute_totals: bool,
}

impl Risc0Executor {
    pub fn new(backend: ProverBackend, groth16_enabled: bool, compute_totals: bool) -> Self {
        // NOTE on CPU vs CUDA dispatch:
        //
        // risc0-zkvm 3.x selects its active prover at *compile time* via the
        // `cuda` cargo feature, not at runtime. `default_prover()` always
        // returns `LocalProver::new("local")` for local proving; whether that
        // LocalProver uses GPU or CPU is decided by whether the contemplant
        // binary was built with `--features enable-risc0-cuda` (which turns
        // on `risc0-zkvm/cuda`, which links CUDA kernels into the binary).
        //
        // The `backend` config field here is informational and drives build
        // selection + capability advertising to the worker registry, but it
        // doesn't flip a runtime switch. Valid RISC0_PROVER env-var values
        // in risc0-zkvm 3.x are "actor" / "bonsai" / "ipc" / "local"; not
        // "cuda"; so we don't set it.
        Self {
            backend,
            groth16_enabled,
            compute_totals,
        }
    }
}

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

    // Execute-phase kick so the watchdog sees work start; the counting
    // layer below then streams real per-segment Prove ticks.
    state
        .proof_store_client
        .proof_progress_update(
            proof_request.request_id,
            ProgressUpdate::indeterminate(ProvePhase::Execute, 1),
        )
        .await;

    let request_id = proof_request.request_id;
    let display = format!(
        "RISC Zero {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );
    let groth16_enabled = executor.groth16_enabled;
    let compute_totals = executor.compute_totals;

    // Bridge the blocking-thread progress ticks (from the tracing layer and
    // the coarse phase markers) to the async proof store. UnboundedSender
    // ::send is sync and non-blocking, so it is safe from a blocking thread.
    let (progress_tx, mut progress_rx) =
        mpsc::unbounded_channel::<ProgressUpdate>();
    {
        let store = state.proof_store_client.clone();
        tokio::spawn(async move {
            while let Some(update) = progress_rx.recv().await {
                store.proof_progress_update(request_id, update).await;
            }
        });
    }

    tokio::task::spawn(async move {
        let start_time = Instant::now();

        // risc0-zkvm's prover calls are CPU-blocking, so we run them on a
        // blocking thread.
        let elf = proof_request.elf;
        let input = proof_request.input;
        let mode = proof_request.mode;
        let wrap_of = proof_request.wrap_of;
        let progress_tx_blocking = progress_tx.clone();

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

            // Scoped subscriber: the counting layer observes risc0's own
            // per-segment `prove_segment` events during proving without
            // altering the (byte-identical) prove call.
            let total = Arc::new(AtomicU64::new(0));
            let layer = R0SegmentLayer {
                count: Arc::new(AtomicU64::new(0)),
                total: total.clone(),
                tx: progress_tx_blocking.clone(),
            };
            let subscriber = tracing_subscriber::registry().with(layer);

            tracing::subscriber::with_default(subscriber, || {
                let prover = default_prover();

                let wrapped_receipt: Receipt = if let Some(source_bytes) = wrap_of {
                    // Two-step wrap: caller has an existing STARK receipt and
                    // wants us to compress it to a smaller form (almost always
                    // Groth16 for onchain verification). This is the Wrap phase.
                    let _ = progress_tx_blocking
                        .send(ProgressUpdate::indeterminate(ProvePhase::Wrap, 1));
                    let source: Receipt = bincode::deserialize(&source_bytes)
                        .map_err(|e| anyhow!("Deserialize source receipt for wrap: {e}"))?;
                    prover
                        .compress(&opts, &source)
                        .map_err(|e| anyhow!("RISC Zero compress (wrap) error: {e}"))?
                } else {
                    // Learn the exact segment total via an execute pass (no
                    // proving) so the per-segment ticks carry a percentage.
                    // Cheap next to proving; on failure fall back to a live
                    // count (proving is never affected).
                    if compute_totals {
                        let env = ExecutorEnv::builder()
                            .write_slice(&input)
                            .build()
                            .map_err(|e| anyhow!("Build RISC Zero ExecutorEnv (total): {e}"))?;
                        match default_executor().execute(env, &elf) {
                            Ok(info) => {
                                let n = info.segments.len() as u64;
                                info!("RISC Zero segment total (execute): {n}");
                                total.store(n, Ordering::Relaxed);
                            }
                            Err(e) => log::warn!(
                                "RISC Zero total computation failed, using live count: {e}"
                            ),
                        }
                    }
                    // Fresh proof: build ExecutorEnv from `input` and prove the ELF.
                    let _ = progress_tx_blocking.send(ProgressUpdate::Phase {
                        phase: ProvePhase::Prove,
                        done: 0,
                        total: total.load(Ordering::Relaxed),
                    });
                    let env = ExecutorEnv::builder()
                        .write_slice(&input)
                        .build()
                        .map_err(|e| anyhow!("Build RISC Zero ExecutorEnv: {e}"))?;
                    let receipt = prover
                        .prove_with_opts(env, &elf, &opts)
                        .map_err(|e| anyhow!("RISC Zero prover error: {e}"))?
                        .receipt;
                    // Succinct/Groth16 modes fold segments into a recursion
                    // tree after core proving; mark the Aggregate phase.
                    if !matches!(mode, Risc0ProofMode::Composite) {
                        let _ = progress_tx_blocking
                            .send(ProgressUpdate::indeterminate(ProvePhase::Aggregate, 1));
                    }
                    receipt
                };

                bincode::serialize(&wrapped_receipt)
                    .map_err(|e| anyhow!("Serialize RISC Zero receipt: {e}"))
            })
        })
        .await
        .map_err(|e| anyhow!("RISC Zero prover join error: {e}"))
        .and_then(|inner| inner);

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        match proof_res {
            Ok(receipt_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(receipt_bytes),
                    )
                    .await;
            }
            Err(e) => {
                let error_msg = format!("Error proving {display} at minute {minutes}: {e}");

                // Report the failure and stay alive. A request whose guest
                // faults (a poisoned input, a program bug) is the request's
                // problem, not the worker's: exiting here lets any bad
                // request kill every contemplant in the fleet one by one.
                // Reporting `Unexecutable` lets hierophant settle the proof
                // per its policy while this worker returns to Idle.
                error!("{error_msg}");

                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;
            }
        }
    });
}
