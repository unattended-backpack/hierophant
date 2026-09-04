//! OpenVM proving via the `openvm-worker` subprocess.
//!
//! The proving implementation lives in `src/openvm-worker` (the 6.5 split:
//! openvm-sdk and sp1-sdk can't share a dependency graph). This executor is a
//! client that spawns the worker once (warm key caches) and drives it over a
//! unix socket, with death-respawn-demote hardening mirroring the SP1 path.

use crate::config::ProverBackend;
use crate::worker_state::WorkerState;

use anyhow::{Result, anyhow};
use log::{error, info, warn};
use network_lib::{
    ContemplantProofStatus, OpenVmProofMode, OpenVmProofRequest, ProgressUpdate, VmKind,
};
use openvm_worker_proto::{ProofMode, WorkerClient, WorkerRequest, WorkerResponse};
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};
use tokio::{sync::mpsc, time::Instant};

/// Consecutive worker transport deaths tolerated before the executor stops
/// respawning `openvm-worker` and drops the worker's OpenVM capability.
const MAX_CONSECUTIVE_WORKER_DEATHS: u32 = 3;

#[derive(Clone)]
pub struct OpenVmExecutor {
    pub backend: ProverBackend,
    pub evm_enabled: bool,
    /// Owns the long-lived openvm-worker child (warm key caches).
    client: Arc<WorkerClient>,
    /// Consecutive transport deaths; reset by any response from a live worker.
    worker_deaths: Arc<AtomicU32>,
    /// Ask the worker to report its segment count (execute_metered) so the
    /// cycle-rate ETA model has a size signal. Config `compute_proof_totals`.
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

    /// True once the worker has died [`MAX_CONSECUTIVE_WORKER_DEATHS`] times
    /// in a row: respawns have stopped and `supported_vms()` omits OpenVM.
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

    let request_id = proof_request.request_id;
    let mode_name = proof_request.mode.as_str().to_string();

    // Executing: no size / ETA yet.
    state
        .proof_store_client
        .proof_progress_update(request_id, ProgressUpdate::executing())
        .await;

    let display = format!(
        "OpenVM {} proof with request id {}",
        proof_request.mode.as_str(),
        request_id
    );

    // The worker reports OpenVM's segment count once (a Size frame) via the
    // call_blocking callback on the blocking thread. Bridge it to an async
    // task that turns it into a live ETA from this contemplant's learned
    // rate; a shared cell keeps it for the completion path to record.
    let size_cell = Arc::new(AtomicU64::new(0));
    let (size_tx, mut size_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
    {
        let store = state.proof_store_client.clone();
        let rate_model = state.rate_model.clone();
        let size_cell = size_cell.clone();
        let mode_name = mode_name.clone();
        tokio::spawn(async move {
            if let Some(segments) = size_rx.recv().await {
                size_cell.store(segments, Ordering::Relaxed);
                let update = match rate_model
                    .lock()
                    .await
                    .estimate_secs(VmKind::OpenVm, &mode_name, segments)
                {
                    Some(est) => ProgressUpdate::proving(segments, est),
                    None => ProgressUpdate::no_history(),
                };
                store.proof_progress_update(request_id, update).await;
            }
        });
    }

    tokio::task::spawn(async move {
        let start_time = Instant::now();

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
            client.call_blocking(&req, |segments| {
                let _ = size_tx.send(segments);
            })
        })
        .await
        .map_err(|e| anyhow!("OpenVM worker join error: {e}"));

        let elapsed_secs = start_time.elapsed().as_secs_f64();
        let minutes = (elapsed_secs / 60.0).round() as u32;

        // Split transport failures (worker health) from request failures.
        let (proof_res, transport_death): (Result<Vec<u8>>, bool) = match call_res {
            Ok(Ok(WorkerResponse::Proof(bytes))) => (Ok(bytes), false),
            Ok(Ok(WorkerResponse::Err(msg))) => {
                (Err(anyhow!("openvm-worker rejected the request: {msg}")), false)
            }
            Ok(Ok(_)) => (
                Err(anyhow!("openvm-worker returned an unexpected response kind")),
                false,
            ),
            Ok(Err(io_err)) => (Err(anyhow!("openvm-worker transport failure: {io_err}")), true),
            Err(join_err) => (Err(join_err), false),
        };

        match proof_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                executor.worker_deaths.store(0, Ordering::Relaxed);
                // Learn this box's throughput for this (vm, mode).
                let segments = size_cell.load(Ordering::Relaxed);
                if segments > 0 {
                    state
                        .rate_model
                        .lock()
                        .await
                        .record(VmKind::OpenVm, &mode_name, segments, elapsed_secs);
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
                warn!("{error_msg}");

                state
                    .proof_store_client
                    .proof_status_update(request_id, ExecutionStatus::Unexecutable.into(), None)
                    .await;

                // A transport death is a WORKER-health failure, not a request
                // fault. The client already killed the child; count deaths
                // and, past the cap, force re-registration so OpenVM is
                // dropped while other VMs keep serving.
                if transport_death {
                    let responded = !executor.worker_permanently_dead();
                    let deaths = executor.worker_deaths.fetch_add(1, Ordering::Relaxed) + 1;
                    if deaths < MAX_CONSECUTIVE_WORKER_DEATHS {
                        error!(
                            "openvm-worker died (consecutive death {deaths} of \
                             {MAX_CONSECUTIVE_WORKER_DEATHS}); it will be respawned on the \
                             next request"
                        );
                    } else if responded {
                        error!(
                            "openvm-worker died {deaths} consecutive times; giving up on \
                             respawns. Forcing re-registration to drop OpenVM from this \
                             worker's capabilities; SP1/R0 capabilities are unaffected."
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
