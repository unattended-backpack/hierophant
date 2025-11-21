use crate::worker_state::WorkerState;
use alloy_primitives::B256;
use log::{error, info, trace, warn};
use network_lib::{
    ContemplantProofStatus,
    messages::{FromContemplantMessage, FromHierophantMessage},
};
use std::ops::ControlFlow;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

pub async fn handle_message_from_hierophant(
    state: WorkerState,
    msg: Message,
    response_sender: mpsc::Sender<FromContemplantMessage>,
    exit_sender: mpsc::Sender<String>,
) -> ControlFlow<(), ()> {
    if let Message::Close(_) = msg {
        info!("Received WebSocket close message from hierophant");
        return ControlFlow::Break(());
    }

    // parse message into FromHierophantMessage
    let msg: FromHierophantMessage = match msg {
        Message::Binary(bytes) => match bincode::deserialize(&bytes) {
            Ok(ws_msg) => ws_msg,
            Err(e) => {
                error!("Error deserializing message: {e}");
                return ControlFlow::Continue(());
            }
        },
        _ => {
            error!("Unsupported message type {msg:?}");
            return ControlFlow::Continue(());
        }
    };

    trace!("Handling FromHierophantMessage {msg}");

    match msg {
        FromHierophantMessage::ProofRequest(proof_request) => {
            crate::proof_executor::execute_proof(state, proof_request, exit_sender).await
        }
        FromHierophantMessage::ProofStatusRequest(request_id) => {
            let proof_status = get_proof_request_status(state, request_id).await;
            if let Err(e) = response_sender
                .send(FromContemplantMessage::ProofStatusResponse(
                    request_id,
                    proof_status,
                ))
                .await
            {
                error!("Error sending proof status response to hierophant: {e}");
                return ControlFlow::Break(());
            }
        }
    };

    ControlFlow::Continue(())
}

async fn get_proof_request_status(
    state: WorkerState,
    request_id: B256,
) -> Option<ContemplantProofStatus> {
    let start = tokio::time::Instant::now();
    let status = match state.proof_store_client.get(request_id).await {
        Some(proof_status) => {
            info!(
                "Received proof status request: {:?}.  Progress is {:?}",
                request_id, proof_status.progress
            );
            Some(proof_status)
        }
        None => {
            warn!(
                "Received proof status request: {request_id:?} but this proof cannot be found"
            );
            None
        }
    };
    let secs = start.elapsed().as_secs_f64();

    if secs > 1.0 {
        info!(
            "Slow execution detected: took {secs} seconds to get proof status for proof request {request_id}"
        );
    }

    status
}
