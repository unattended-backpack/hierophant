use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Context, Result, anyhow};
use log::info;
use network_lib::{ContemplantProofStatus, ProgressUpdate, Risc0ProofMode, Risc0ProofRequest};
use risc0_zkvm::{ExecutorEnv, ProverOpts, Receipt, default_prover};
use sp1_sdk::network::proto::network::ExecutionStatus;
use tokio::{sync::mpsc, time::Instant};

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
}

impl Risc0Executor {
    pub fn new(backend: ProverBackend, groth16_enabled: bool) -> Self {
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
        }
    }
}

pub(super) async fn execute(
    state: WorkerState,
    executor: Risc0Executor,
    proof_request: Risc0ProofRequest,
    exit_sender: mpsc::Sender<String>,
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

    // CPU-only v1: no cycle-accurate progress tracking is wired up yet.
    // Push an initial non-zero Execution update so hierophant's progress
    // watchdog knows work has started.
    state
        .proof_store_client
        .proof_progress_update(proof_request.request_id, ProgressUpdate::Execution(1))
        .await;

    let request_id = proof_request.request_id;
    let display = format!(
        "RISC Zero {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );
    let groth16_enabled = executor.groth16_enabled;

    tokio::task::spawn(async move {
        let start_time = Instant::now();

        // risc0-zkvm's prover calls are CPU-blocking, so we run them on a
        // blocking thread.
        let elf = proof_request.elf;
        let input = proof_request.input;
        let mode = proof_request.mode;
        let wrap_of = proof_request.wrap_of;

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
                // Two-step wrap: caller has an existing STARK receipt and
                // wants us to compress it to a smaller form (almost always
                // Groth16 for onchain verification).
                let source: Receipt = bincode::deserialize(&source_bytes)
                    .map_err(|e| anyhow!("Deserialize source receipt for wrap: {e}"))?;
                prover
                    .compress(&opts, &source)
                    .map_err(|e| anyhow!("RISC Zero compress (wrap) error: {e}"))?
            } else {
                // Fresh proof: build ExecutorEnv from `input` and prove the ELF.
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

                // Match the SP1 path: on prover error the contemplant exits so
                // hierophant reassigns the proof to a fresh worker.
                exit_sender
                    .send(error_msg)
                    .await
                    .context("Send exit error message to main thread")
                    .unwrap();

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
