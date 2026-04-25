mod assessor;
mod risc0_executor;
mod sp1_executor;

pub use risc0_executor::Risc0Executor;
pub use sp1_executor::Sp1Executor;

use crate::worker_state::WorkerState;
use log::error;
use network_lib::ContemplantProofRequest;
use tokio::sync::mpsc;

// Entry point invoked by message_handler when the hierophant sends a
// ProofRequest.  Routes to the right VM-specific executor based on the
// request's enum discriminant.
pub async fn execute_proof(
    state: WorkerState,
    proof_request: ContemplantProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
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
    }
}
