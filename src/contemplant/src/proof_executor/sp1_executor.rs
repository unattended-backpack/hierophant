use crate::worker_state::{ActiveSp1Prover, WorkerState};

use anyhow::anyhow;
use log::{error, info};
use network_lib::{
    ContemplantProofStatus, ProgressUpdate, ProvePhase, Sp1ProofRequest, to_proof_from_network,
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

/// Consecutive CUDA-backend transport deaths tolerated before the
/// executor stops respawning `sp1-gpu-server` and permanently drops the
/// worker's SP1 capability (advertised at the next ws registration).
/// Each respawn is cheap and rescues transient deaths (a co-tenant OOM
/// spike, a driver hiccup); a box that kills the server this many
/// proofs in a row is structurally unfit for SP1 work.
const MAX_CONSECUTIVE_CUDA_DEATHS: u32 = 3;

#[derive(Clone)]
pub struct Sp1Executor {
    /// Swappable so a dead CUDA backend (`sp1-gpu-server` is spawned
    /// ONCE by sp1-sdk and never respawned on crash) can be rebuilt
    /// in-process instead of permanently breaking every future SP1
    /// proof until a worker restart.
    pub active_prover: Arc<RwLock<ActiveSp1Prover>>,
    pub mock_prover: Arc<MockProver>,
    pub progress_tracking_available: bool,
    /// Consecutive CUDA transport deaths; reset to zero by any
    /// successful proof.
    pub cuda_deaths: Arc<AtomicU32>,
}

impl Sp1Executor {
    /// True once the CUDA backend has died [`MAX_CONSECUTIVE_CUDA_DEATHS`]
    /// times in a row: respawns have stopped, and `supported_vms()`
    /// omits SP1 from the worker's capabilities at its next
    /// registration.
    pub fn backend_permanently_dead(&self) -> bool {
        self.cuda_deaths.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_CUDA_DEATHS
    }
}

/// Transport-level death signature of the local `sp1-gpu-server` child:
/// distinguishes "the backend process is gone" (worker-health problem —
/// respawn) from a guest fault (the request's problem — report and move
/// on). Matches the CudaClientError socket failures observed live when
/// the cgroup OOM-killer took the server.
fn is_cuda_backend_death(msg: &str) -> bool {
    msg.contains("CudaClientError")
        && (msg.contains("early eof")
            || msg.contains("Broken pipe")
            || msg.contains("UnexpectedEof")
            || msg.contains("Connection refused")
            || msg.contains("Failed to write the request")
            || msg.contains("Failed to read the response"))
}

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode,
// and SP1Stdin provided by the Hierophant
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

    // SP1 progress comes from the sp1-gpu-server's inherited stderr (see
    // sp1_progress): register this proof so the tap attributes the
    // server's lines to it, and bridge parsed ticks to the proof store.
    // UnboundedSender::send is sync, so the tap's reader thread can push
    // without an async context. Dropping the sender (clear_active on
    // completion) ends the drain task.
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
    {
        let store = state.proof_store_client.clone();
        tokio::spawn(async move {
            while let Some(update) = progress_rx.recv().await {
                store.proof_progress_update(request_id, update).await;
            }
        });
    }
    crate::sp1_progress::set_active(request_id, progress_tx);
    state
        .proof_store_client
        .proof_progress_update(
            request_id,
            ProgressUpdate::indeterminate(ProvePhase::Execute, 1),
        )
        .await;

    let display = format!(
        "SP1 {} proof with request id {}",
        proof_request.mode.as_str_name(),
        request_id
    );
    tokio::task::spawn(async move {

        let start_time = Instant::now();
        let mock = proof_request.mock;
        let stdin = &proof_request.sp1_stdin;

        // Map the request's wire-level mode onto the SDK's mode once; 6.x
        // prove requests take the mode as a builder argument, replacing the
        // per-mode method chains of 5.x.
        let sp1_mode = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => None,
            ProofMode::Core => Some(SP1ProofMode::Core),
            ProofMode::Compressed => Some(SP1ProofMode::Compressed),
            ProofMode::Plonk => Some(SP1ProofMode::Plonk),
            ProofMode::Groth16 => Some(SP1ProofMode::Groth16),
        };

        // setup() + prove() must stay within one prover branch: each 6.x
        // prover has its own proving-key type (the cuda prover's key is a
        // remote handle to state held by the sp1-gpu-server, re-created by
        // each setup() call), so the pair can't be hoisted out and shared
        // across branches the way the 5.x code did.
        let elf = proof_request.elf.clone();
        // Clone the prover handle out under a brief read lock rather
        // than holding the lock across the (potentially minutes-long)
        // prove: a concurrent backend rebuild only needs the write lock
        // between proofs.
        let active_prover = executor.active_prover.read().await.clone();
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

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        let proof_bytes_res = proof_res.and_then(|proof| {
            let network_proof: ProofFromNetwork = to_proof_from_network(proof);
            bincode::serialize(&network_proof).map_err(|e| anyhow!("Error serializing proof {e}"))
        });

        match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                executor.cuda_deaths.store(0, Ordering::Relaxed);
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
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

                // A CUDA transport death is a WORKER-health failure, not a
                // request fault: sp1-sdk spawns `sp1-gpu-server` once and
                // never respawns it, so without intervention every future
                // SP1 proof on this worker fails instantly. Rebuild the
                // prover (which spawns a fresh server) up to
                // [`MAX_CONSECUTIVE_CUDA_DEATHS`] times; past that, stop —
                // `supported_vms()` then omits SP1 at the next ws
                // registration and the worker keeps serving its other VMs.
                // Each death leaves one defunct child (sp1-sdk never
                // reaps); bounded by the cap, and deliberately not reaped
                // here — a blanket waitpid(-1) could steal exit statuses
                // from unrelated children like the Groth16 shim.
                if is_cuda_backend_death(&error_msg)
                    && matches!(&active_prover, ActiveSp1Prover::Cuda(_))
                {
                    let deaths =
                        executor.cuda_deaths.fetch_add(1, Ordering::Relaxed) + 1;
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
                            "SP1 CUDA backend died {deaths} consecutive times; giving \
                             up on respawns. Forcing re-registration to drop SP1 from \
                             this worker's capabilities; R0/OpenVM capabilities are \
                             unaffected."
                        );
                        state.reconnect.notify_one();
                    }
                }
            }
        };

        // Stop attributing gpu-server stderr lines to this proof and end
        // the progress drain task (dropping the tap's sender).
        crate::sp1_progress::clear_active(request_id);
    });
}
