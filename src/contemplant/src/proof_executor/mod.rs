mod openvm_executor;
mod risc0_executor;
mod sp1_executor;

pub use openvm_executor::OpenVmExecutor;
pub use risc0_executor::Risc0Executor;
pub use sp1_executor::Sp1Executor;

use crate::worker_state::WorkerState;
use log::{error, info};
use network_lib::{ContemplantProofRequest, VmKind};
use tokio::sync::mpsc;

// Entry point invoked by message_handler when the hierophant sends a
// ProofRequest.  Routes to the right VM-specific executor based on the
// request's enum discriminant.
pub async fn execute_proof(
    state: WorkerState,
    proof_request: ContemplantProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
    // Single hot backend per contemplant. Proofs are serial (the registry
    // dispatches one at a time), and each zkVM backend can hold large amounts
    // of GPU VRAM resident (notably OpenVM's ~15.5GB halo2 key), so one GPU
    // cannot keep all three loaded at once. Keep the current zkVM hot for
    // repeat proofs of that VM; when the requested VM differs, evict the
    // previously hot backend to free VRAM first. The routed executor re-warms
    // its own backend on this proof.
    let requested_vm = match &proof_request {
        ContemplantProofRequest::Sp1(_) => VmKind::Sp1,
        ContemplantProofRequest::Risc0(_) => VmKind::Risc0,
        ContemplantProofRequest::OpenVm(_) => VmKind::OpenVm,
    };
    {
        let mut hot = state.resident_vm.lock().await;
        if let Some(current) = *hot {
            if current != requested_vm {
                info!(
                    "Switching hot zkVM {current:?} -> {requested_vm:?}; releasing {current:?} GPU resources"
                );
                match current {
                    VmKind::Sp1 => {
                        if let Some(e) = &state.sp1_executor {
                            e.release_gpu().await;
                        }
                    }
                    VmKind::Risc0 => {
                        if let Some(e) = &state.risc0_executor {
                            e.release_gpu();
                        }
                    }
                    VmKind::OpenVm => {
                        if let Some(e) = &state.openvm_executor {
                            e.release_gpu();
                        }
                    }
                }
            }
        }
        *hot = Some(requested_vm);
    }

    match proof_request {
        ContemplantProofRequest::Sp1(req) => match state.sp1_executor.clone() {
            Some(executor) => {
                sp1_executor::execute(state, executor, req, exit_sender).await;
            }
            None => {
                let msg = format!(
                    "Received SP1 proof request {} but this contemplant does not serve SP1",
                    req.request_id
                );
                error!("{msg}");
                let _ = exit_sender.send(msg).await;
            }
        },
        ContemplantProofRequest::Risc0(req) => match state.risc0_executor.clone() {
            Some(executor) => {
                risc0_executor::execute(state, executor, req, exit_sender).await;
            }
            None => {
                let msg = format!(
                    "Received RISC Zero proof request {} but this contemplant does not serve RISC Zero",
                    req.request_id
                );
                error!("{msg}");
                let _ = exit_sender.send(msg).await;
            }
        },
        ContemplantProofRequest::OpenVm(req) => match state.openvm_executor.clone() {
            Some(executor) => {
                openvm_executor::execute(state, executor, req, exit_sender).await;
            }
            None => {
                let msg = format!(
                    "Received OpenVM proof request {} but this contemplant does not serve OpenVM",
                    req.request_id
                );
                error!("{msg}");
                let _ = exit_sender.send(msg).await;
            }
        },
    }
}
