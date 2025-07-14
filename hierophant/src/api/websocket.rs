use crate::worker_registry::WorkerRegistryClient;
use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use network_lib::messages::{FromContemplantMessage, FromHierophantMessage};
use std::net::SocketAddr;
use std::ops::ControlFlow;
use tokio::sync::mpsc;

use axum::extract::ws::{Message, WebSocket};

// Actual websocket statemachine (one will be spawned per connection)
// Will spawn 2 tasks for each connection:
//      1. Receiving messages (Register, ProofStatusResponse, Heartbeat, etc)
//      2. Sending messages (ProofRequest, ProofStatusRequest, etc)
pub async fn handle_socket(
    socket: WebSocket,
    who: SocketAddr,
    worker_registry_client: WorkerRegistryClient,
) {
    // By splitting socket we can send and receive at the same time.
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Channel for sending/ receiving messages from the worker_registry from/to the contemplant
    let (from_hierophant_msg_sender, mut from_hierophant_msg_receiver) =
        mpsc::channel::<FromHierophantMessage>(100);

    // this thread solely sends ws messages back to the contemplant
    let mut send_task = tokio::spawn(async move {
        // wait to receive FromHierophantMessage messages from worker_registry
        while let Some(ws_msg) = from_hierophant_msg_receiver.recv().await {
            // serialize message
            let ws_msg_bytes = match bincode::serialize(&ws_msg) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let error_msg = format!("Error serializing message {ws_msg}: {e}");
                    error!("{error_msg}");
                    // skip this message
                    continue;
                }
            };

            // send the message to the Contemplant via ws
            let msg = Message::Binary(ws_msg_bytes);
            if let Err(e) = ws_sender.send(msg).await {
                error!("Error sending message to contemplant {who}: {e}");
                break;
            }
        }

        // when the above while loop exits for some reason, send a close message to the contemplant
        // because this connection is shutting down
        if let Err(e) = ws_sender.send(Message::Close(None)).await {
            warn!("Could not send Close to contemplant due to {e}");
        }
    });

    // This thread solely receives messages from contemplant and sends them to the worker_registry
    // to be handled
    let mut recv_task = tokio::spawn(async move {
        // wait to receive Messages from the Contemplant's ws
        while let Some(msg_result) = ws_receiver.next().await {
            match msg_result {
                Ok(msg) => {
                    if handle_message_from_contemplant(
                        msg,
                        who,
                        &worker_registry_client,
                        from_hierophant_msg_sender.clone(),
                    )
                    .await
                    .is_break()
                    {
                        break;
                    }
                }
                Err(e) => {
                    error!("Error receiving message from contemplant {who}: {e}");
                    break;
                }
            }
        }
    });

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        result = (&mut send_task) => {
            match result {
                Ok(_) => debug!("Send task completed normally"),
                Err(e) => error!("Send task failed with error: {e}"),
            }
            debug!("Send task exited, aborting recv task");
            recv_task.abort();
        },
        result = (&mut recv_task) => {
            match result {
                Ok(_) => debug!("Recv task completed normally"),
                Err(e) => error!("Recv task failed with error: {e}"),
            }
            debug!("Recv task exited, aborting send task");
            send_task.abort();
        }
    }

    info!("Websocket context {who} destroyed");
}

// processes a message from the contemplant
// returns Continue or Break to decide if the ws connection should be cut or not
async fn handle_message_from_contemplant(
    msg: Message,
    socket_addr: SocketAddr,
    worker_registry_client: &WorkerRegistryClient,
    // channel that forwards messages to the contemplant via ws
    from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
) -> ControlFlow<(), ()> {
    if let Message::Close(_) = msg {
        info!("Received WebSocket close message from contemplant {socket_addr}");
        return ControlFlow::Break(());
    }

    // parse message into FromContemplantMessage
    let msg: FromContemplantMessage = match msg {
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

    let worker_addr = format!("{}:{}", socket_addr.ip(), socket_addr.port());

    // handle it appropriately
    match msg {
        FromContemplantMessage::Register(worker_register_info) => {
            // Return this result.  If the worker is on a wrong version this will be an error
            // and we should close the connection with the worker.
            worker_registry_client
                .worker_ready(worker_addr, worker_register_info, from_hierophant_sender)
                .await
        }
        FromContemplantMessage::ProofStatusResponse(request_id, maybe_proof_status) => {
            worker_registry_client
                .proof_status_response(request_id, maybe_proof_status)
                .await
        }
        FromContemplantMessage::Heartbeat => {
            // if we receive a heartbeat from a worker that we evicted, close the connection to it
            // by returning an error from here
            worker_registry_client.heartbeat(worker_addr).await
        }
    }
}
