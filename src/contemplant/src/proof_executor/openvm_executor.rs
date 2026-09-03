//! OpenVM proving via the `openvm-worker` subprocess.
//!
//! The proving implementation that used to live in this file (openvm-sdk
//! keygen + prove, key caches, ~/.openvm artifact loading) moved verbatim
//! into `src/openvm-worker` at the 6.5 split: openvm-sdk exact-pins the
//! upstream plonky3 crates while sp1-sdk 6.2.2+ pins the Succinct forks
//! at the same 0.4 minor, so the two stacks cannot share this binary's
//! dependency graph. This executor is now a client that spawns the
//! worker once (warm key caches live for the worker's lifetime, exactly
//! like the old in-process statics) and drives it over a unix socket,
//! with the same death-respawn-demote hardening the SP1 executor applies
//! to its sp1-gpu-server child.

use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Result, anyhow};
use log::{error, info, warn};
use network_lib::{
    ContemplantProofStatus, OpenVmProofMode, OpenVmProofRequest, ProgressUpdate, ProvePhase,
};
use openvm_worker_proto::{
    ProofMode, ProvePhase as WireProvePhase, WorkerClient, WorkerRequest, WorkerResponse,
};
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use tokio::{sync::mpsc, time::Instant};

/// Consecutive worker transport deaths tolerated before the executor
/// stops respawning `openvm-worker` and permanently drops the worker's
/// OpenVM capability (advertised at the next ws registration). Mirrors
/// MAX_CONSECUTIVE_CUDA_DEATHS on the SP1 side: respawns rescue
/// transient deaths (an OOM spike mid-keygen, a driver hiccup); a box
/// that kills the worker this many proofs in a row is structurally
/// unfit for OpenVM work.
const MAX_CONSECUTIVE_WORKER_DEATHS: u32 = 3;

#[derive(Clone)]
pub struct OpenVmExecutor {
    pub backend: ProverBackend,
    pub evm_enabled: bool,
    /// Owns the openvm-worker child; spawned on first use and respawned
    /// by the client after a death. Shared so every proof reuses the
    /// same long-lived worker (whose in-process key caches carry the
    /// warmth the old in-process statics had).
    client: Arc<WorkerClient>,
    /// Consecutive transport deaths; reset to zero by any response from
    /// a live worker (including request-level failures).
    worker_deaths: Arc<AtomicU32>,
    /// Ask the worker to compute the exact segment total (execute_metered)
    /// so progress is a percentage rather than a bare count.
    compute_totals: bool,
}

impl OpenVmExecutor {
    pub fn new(backend: ProverBackend, evm_enabled: bool, compute_totals: bool) -> Self {
        let socket = std::env::temp_dir().join(format!(
            "openvm-worker-contemplant-{}.sock",
            std::process::id()
        ));
        let mut args = vec![
            "--backend".to_string(),
            match backend {
                ProverBackend::Cpu => "cpu".to_string(),
                ProverBackend::Cuda => "cuda".to_string(),
            },
        ];
        if evm_enabled {
            args.push("--evm".to_string());
        }
        let client = Arc::new(WorkerClient::new(socket, args));
        // Spawn eagerly so a missing/broken worker binary is loud at
        // startup rather than at the first assigned proof; failures here
        // are retried lazily by call_blocking, so only log.
        if let Err(e) = client.ensure_running() {
            error!("failed to spawn openvm-worker at startup (will retry per request): {e}");
        }
        Self {
            backend,
            evm_enabled,
            client,
            worker_deaths: Arc::new(AtomicU32::new(0)),
            compute_totals,
        }
    }

    /// True once the worker has died [`MAX_CONSECUTIVE_WORKER_DEATHS`]
    /// times in a row: respawns have stopped, and `supported_vms()`
    /// omits OpenVM from the worker's capabilities at its next
    /// registration.
    pub fn worker_permanently_dead(&self) -> bool {
        self.worker_deaths.load(Ordering::Relaxed) >= MAX_CONSECUTIVE_WORKER_DEATHS
    }
}

fn to_proto_mode(mode: OpenVmProofMode) -> ProofMode {
    match mode {
        OpenVmProofMode::App => ProofMode::App,
        OpenVmProofMode::Stark => ProofMode::Stark,
        OpenVmProofMode::Evm => ProofMode::Evm,
    }
}

// The worker speaks the proto ProvePhase mirror (proto cannot link
// network-lib); map it back to the wire enum for the proof store.
fn from_proto_phase(phase: WireProvePhase) -> ProvePhase {
    match phase {
        WireProvePhase::Execute => ProvePhase::Execute,
        WireProvePhase::Prove => ProvePhase::Prove,
        WireProvePhase::Aggregate => ProvePhase::Aggregate,
        WireProvePhase::Wrap => ProvePhase::Wrap,
    }
}

pub(super) async fn execute(
    state: WorkerState,
    executor: OpenVmExecutor,
    proof_request: OpenVmProofRequest,
    // Retained for genuinely fatal, environment-level errors; per-proof
    // failures report `Unexecutable` and keep the worker alive.
    _exit_sender: mpsc::Sender<String>,
) {
    info!(
        "Received OpenVM proof request {} (mode {}, backend {:?})",
        proof_request.request_id,
        proof_request.mode.as_str(),
        executor.backend,
    );

    let initial_status = ContemplantProofStatus::unexecuted();
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    // Kick off with an Execute-phase tick so hierophant's progress
    // watchdog sees work has started; the worker then streams real
    // per-segment Prove ticks over the socket (see below).
    state
        .proof_store_client
        .proof_progress_update(
            proof_request.request_id,
            ProgressUpdate::indeterminate(ProvePhase::Execute, 1),
        )
        .await;

    let request_id = proof_request.request_id;
    let display = format!(
        "OpenVM {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );

    // Bridge the worker's synchronous progress callback (fired on the
    // blocking thread) to the async proof store. UnboundedSender::send is
    // sync and non-blocking, so it is safe to call from the callback.
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

    tokio::task::spawn(async move {
        let start_time = Instant::now();

        // The worker call blocks for the whole proving run; keep it off
        // the async runtime. NOTE: like the RISC Zero path, `mock` is
        // accepted but not implemented for OpenVM; a real proof is
        // produced regardless.
        let req = WorkerRequest::Prove {
            request_id: request_id.to_string(),
            mode: to_proto_mode(proof_request.mode),
            elf: proof_request.elf,
            app_config_toml: proof_request.app_config_toml,
            input: proof_request.input,
            with_total: executor.compute_totals,
        };
        let client = executor.client.clone();
        let call_res = tokio::task::spawn_blocking(move || {
            client.call_blocking(&req, |phase, done, total| {
                let _ = progress_tx.send(ProgressUpdate::Phase {
                    phase: from_proto_phase(phase),
                    done,
                    total,
                });
            })
        })
        .await
        .map_err(|e| anyhow!("OpenVM worker join error: {e}"));

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        // Split transport failures (worker health) from request failures
        // (this proof's problem) before the shared reporting below.
        let (proof_res, transport_death): (Result<Vec<u8>>, bool) = match call_res {
            Ok(Ok(WorkerResponse::Proof(bytes))) => (Ok(bytes), false),
            Ok(Ok(WorkerResponse::Err(msg))) => {
                (Err(anyhow!("openvm-worker rejected the request: {msg}")), false)
            }
            Ok(Ok(_)) => (
                Err(anyhow!("openvm-worker returned an unexpected response kind")),
                false,
            ),
            Ok(Err(io_err)) => (
                Err(anyhow!("openvm-worker transport failure: {io_err}")),
                true,
            ),
            Err(join_err) => (Err(join_err), false),
        };

        match proof_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                executor.worker_deaths.store(0, Ordering::Relaxed);
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
                warn!("{error_msg}");

                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;

                // A transport death is a WORKER-health failure, not a
                // request fault. The client already killed the child, so
                // the next request respawns it; count consecutive deaths
                // and, past the cap, force re-registration so
                // `supported_vms()` drops OpenVM while other VMs keep
                // serving (exact mirror of the SP1 CUDA arrangement).
                if transport_death {
                    let responded = !executor.worker_permanently_dead();
                    let deaths =
                        executor.worker_deaths.fetch_add(1, Ordering::Relaxed) + 1;
                    if deaths < MAX_CONSECUTIVE_WORKER_DEATHS {
                        error!(
                            "openvm-worker died (consecutive death {deaths} of \
                             {MAX_CONSECUTIVE_WORKER_DEATHS}); it will be respawned on \
                             the next request"
                        );
                    } else if responded {
                        error!(
                            "openvm-worker died {deaths} consecutive times; giving up \
                             on respawns. Forcing re-registration to drop OpenVM from \
                             this worker's capabilities; SP1/R0 capabilities are \
                             unaffected."
                        );
                        state.reconnect.notify_one();
                    }
                } else {
                    // The worker answered (with a failure): it is alive.
                    executor.worker_deaths.store(0, Ordering::Relaxed);
                }
            }
        }
    });
}
