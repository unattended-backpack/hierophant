use crate::worker_state::{ActiveSp1Prover, WorkerState};

use anyhow::anyhow;
use log::{error, info};
use network_lib::{
    ContemplantProofStatus, ProgressUpdate, Sp1ProofRequest, VmKind, to_proof_from_network,
};
use sp1_sdk::proof::ProofFromNetwork;
use sp1_sdk::{
    MockProver, ProveRequest, Prover, SP1ProofMode, SP1ProofWithPublicValues,
    network::proto::base::types::{ExecutionStatus, ProofMode},
};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use tokio::{
    sync::{RwLock, mpsc},
    time::Instant,
};

/// Consecutive CUDA-backend transport deaths tolerated before the executor
/// stops respawning `sp1-gpu-server` and permanently drops the worker's SP1
/// capability (advertised at the next ws registration).
const MAX_CONSECUTIVE_CUDA_DEATHS: u32 = 3;

#[derive(Clone)]
pub struct Sp1Executor {
    /// Swappable so a dead CUDA backend (`sp1-gpu-server` is spawned ONCE by
    /// sp1-sdk and never respawned on crash) can be rebuilt in-process.
    pub active_prover: Arc<RwLock<ActiveSp1Prover>>,
    pub mock_prover: Arc<MockProver>,
    /// Consecutive CUDA transport deaths; reset to zero by any successful proof.
    pub cuda_deaths: Arc<AtomicU32>,
    /// Whether to run the quick execute pass that yields the cycle count for
    /// the cycle-rate ETA (config `compute_proof_totals`). When false, proofs
    /// report `Estimating` with no ETA and skip the extra execution.
    pub compute_totals: bool,
}

impl Sp1Executor {
    /// True once the CUDA backend has died [`MAX_CONSECUTIVE_CUDA_DEATHS`]
    /// times in a row: respawns have stopped and `supported_vms()` omits SP1.
    pub fn backend_permanently_dead(&self) -> bool {
        self.cuda_deaths.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_CUDA_DEATHS
    }
}

/// Transport-level death signature of the local `sp1-gpu-server` child.
fn is_cuda_backend_death(msg: &str) -> bool {
    msg.contains("CudaClientError")
        && (msg.contains("early eof")
            || msg.contains("Broken pipe")
            || msg.contains("UnexpectedEof")
            || msg.contains("Connection refused")
            || msg.contains("Failed to write the request")
            || msg.contains("Failed to read the response"))
}

// Uses the CudaProver or MockProver to prove the request. Progress is the
// cycle-rate ETA model (see rate_model): a quick standalone execute() pass
// yields the cycle count, from which this contemplant's learned per-(vm,mode)
// rate produces a live ETA. This replaces the old sp1-gpu-server stderr tap,
// which was opaque, unversioned, and broke silently across SP1 bumps.
pub(super) async fn execute(
    state: WorkerState,
    executor: Sp1Executor,
    proof_request: Sp1ProofRequest,
    // Retained for genuinely fatal, environment-level errors; per-proof
    // failures report `Unexecutable` and keep the worker alive.
    _exit_sender: mpsc::Sender<String>,
) {
    info!(
        "Received SP1 proof request {} (mode {})",
        proof_request.request_id,
        proof_request.mode.as_str_name()
    );

    let initial_status = ContemplantProofStatus::unexecuted();
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    let request_id = proof_request.request_id;
    let mode = proof_request.mode.as_str_name().to_string();

    // Executing: the guest has not run yet, so the cycle count (the ETA
    // input) is unknown. No estimate until the execute pass below.
    state
        .proof_store_client
        .proof_progress_update(request_id, ProgressUpdate::executing())
        .await;

    let display = format!(
        "SP1 {} proof with request id {}",
        proof_request.mode.as_str_name(),
        request_id
    );
    tokio::task::spawn(async move {
        // Total contemplant-side wall-clock; the ETA and the recorded sample
        // both cover execute + prove.
        let start_time = Instant::now();
        let mock = proof_request.mock;
        let stdin = &proof_request.sp1_stdin;

        let sp1_mode = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => None,
            ProofMode::Core => Some(SP1ProofMode::Core),
            ProofMode::Compressed => Some(SP1ProofMode::Compressed),
            ProofMode::Plonk => Some(SP1ProofMode::Plonk),
            ProofMode::Groth16 => Some(SP1ProofMode::Groth16),
        };

        let elf = proof_request.elf.clone();
        // Clone the prover handle out under a brief read lock rather than
        // holding it across the (minutes-long) prove.
        let active_prover = executor.active_prover.read().await.clone();

        // Cycle count + ETA. SP1's prove is one opaque bulk call, so the
        // cycle count is learned from a quick standalone execute() pass
        // (cheap relative to proving); then publish an estimate from this
        // contemplant's learned rate, or NoHistory if it has never proven
        // this (vm, mode) before. Skipped for mock proofs (instant) and when
        // `compute_proof_totals` is off (no ETA, no extra execution).
        let mut cycles: Option<u64> = None;
        if !mock && executor.compute_totals {
            // NOTE(build): sp1-sdk `Prover::execute(elf, stdin)` returns an
            // ExecuteRequest builder resolving to (SP1PublicValues,
            // ExecutionReport); `total_instruction_count()` is the cycle count.
            let exec_res = match &active_prover {
                ActiveSp1Prover::Cpu(p) => p.execute(elf.clone().into(), stdin.clone()).await,
                ActiveSp1Prover::Cuda(p) => p.execute(elf.clone().into(), stdin.clone()).await,
            };
            match exec_res {
                Ok((_public_values, report)) => {
                    let c = report.total_instruction_count();
                    cycles = Some(c);
                    let update = match state
                        .rate_model
                        .lock()
                        .await
                        .estimate_secs(VmKind::Sp1, &mode, c)
                    {
                        Some(est) => ProgressUpdate::proving(c, est),
                        None => ProgressUpdate::no_history(),
                    };
                    state
                        .proof_store_client
                        .proof_progress_update(request_id, update)
                        .await;
                }
                Err(e) => {
                    // Could not even count cycles; the prove below will
                    // surface the real error. Stay Estimating(Executing).
                    error!("SP1 execute pass for {display} failed (no ETA): {e}");
                }
            }
        }

        let proof_res: anyhow::Result<SP1ProofWithPublicValues> = match sp1_mode {
            None => Err(anyhow!("UnspecifiedProofMode")),
            Some(sp1_mode) => {
                if mock {
                    async {
                        let pk = executor.mock_prover.setup(elf.into()).await?;
                        executor
                            .mock_prover
                            .prove(&pk, stdin.clone())
                            .mode(sp1_mode)
                            .await
                    }
                    .await
                } else {
                    match &active_prover {
                        ActiveSp1Prover::Cpu(p) => {
                            async {
                                let pk = p.setup(elf.into()).await?;
                                Ok(p.prove(&pk, stdin.clone()).mode(sp1_mode).await?)
                            }
                            .await
                        }
                        ActiveSp1Prover::Cuda(p) => {
                            async {
                                let pk = p.setup(elf.into()).await?;
                                Ok(p.prove(&pk, stdin.clone()).mode(sp1_mode).await?)
                            }
                            .await
                        }
                    }
                }
            }
        };

        let elapsed_secs = start_time.elapsed().as_secs_f64();
        let minutes = (elapsed_secs / 60.0).round() as u32;

        let proof_bytes_res = proof_res.and_then(|proof| {
            let network_proof: ProofFromNetwork = to_proof_from_network(proof);
            bincode::serialize(&network_proof).map_err(|e| anyhow!("Error serializing proof {e}"))
        });

        match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                executor.cuda_deaths.store(0, Ordering::Relaxed);
                // Learn this box's throughput for this (vm, mode) so the next
                // proof of a similar size gets a live ETA.
                if let Some(c) = cycles {
                    state
                        .rate_model
                        .lock()
                        .await
                        .record(VmKind::Sp1, &mode, c, elapsed_secs);
                }
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
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

                // A CUDA transport death is a WORKER-health failure, not a
                // request fault: rebuild the prover (spawns a fresh server)
                // up to the cap; past that, demote SP1 at the next ws
                // registration.
                if is_cuda_backend_death(&error_msg)
                    && matches!(&active_prover, ActiveSp1Prover::Cuda(_))
                {
                    let deaths = executor.cuda_deaths.fetch_add(1, Ordering::Relaxed) + 1;
                    if deaths < MAX_CONSECUTIVE_CUDA_DEATHS {
                        error!(
                            "SP1 CUDA backend died (consecutive death {deaths} of \
                             {MAX_CONSECUTIVE_CUDA_DEATHS}); respawning sp1-gpu-server"
                        );
                        let rebuilt = crate::worker_state::build_sp1_active(
                            crate::config::ProverBackend::Cuda,
                            &None,
                        )
                        .await;
                        *executor.active_prover.write().await = rebuilt;
                        info!("SP1 CUDA backend respawned");
                    } else {
                        error!(
                            "SP1 CUDA backend died {deaths} consecutive times; giving up on \
                             respawns. Forcing re-registration to drop SP1 from this worker's \
                             capabilities; R0/OpenVM capabilities are unaffected."
                        );
                        state.reconnect.notify_one();
                    }
                }
            }
        };
    });
}
