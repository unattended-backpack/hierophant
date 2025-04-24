use crate::proof_router::WorkerRegistryClient;
use anyhow::Result;
use futures_util::stream::FuturesUnordered;
use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use network_lib::{FromContemplantMessage, FromHierophantMessage};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

use axum::{
    Router,
    body::Bytes,
    extract::{
        ConnectInfo, DefaultBodyLimit, Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::any,
};

// Actual websocket statemachine (one will be spawned per connection)
// Will spawn 2 tasks for each connection:
//      1. Receiving messages (Register, ProofStatusResponse, Heartbeat, etc)
//      2. Sending messages (ProofRequest, ProofStatusRequest, etc)
pub async fn handle_socket(
    mut socket: WebSocket,
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
                    let error_msg = format!("Error serializing message {}: {e}", ws_msg);
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
            warn!("Could not send Close due to {e}, probably it is ok?");
        }
    });

    // This thread solely receives messages from contemplant and sends them to the worker_registry
    // to be handled
    let mut recv_task = tokio::spawn(async move {
        // wait to receive Messages from the Contemplant's ws
        while let Some(Ok(msg)) = ws_receiver.next().await {
            if let Err(e) = handle_message_from_contemplant(
                msg,
                who,
                &worker_registry_client,
                from_hierophant_msg_sender.clone(),
            )
            .await
            {
                error!("{e}");
                // TODO: should we break?
                // How best to close connection to the client?
            }
        }
    });

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
        },
        _ = (&mut recv_task) => {
            send_task.abort();
        }
    }

    info!("Websocket context {who} destroyed");
}

// processes a message from the contemplant
async fn handle_message_from_contemplant(
    msg: Message,
    socket_addr: SocketAddr,
    worker_registry_client: &WorkerRegistryClient,
    // channel that forwards messages to the contemplant via ws
    from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
) -> Result<()> {
    // parse message into FromContemplantMessage
    let msg: FromContemplantMessage = match msg {
        Message::Binary(bytes) => match bincode::deserialize(&bytes) {
            Ok(ws_msg) => ws_msg,
            Err(e) => {
                error!("Error deserializing message: {e}");
                return Ok(());
            }
        },
        _ => {
            error!("Unsupported message type {:?}", msg);
            return Ok(());
        }
    };

    let worker_addr = socket_addr.ip().to_string();

    // handle it appropriately
    match msg {
        FromContemplantMessage::Register(worker_register_info) => {
            // TODO: how to best handle client
            worker_registry_client
                .worker_ready(worker_addr, worker_register_info, from_hierophant_sender)
                .await?;
        }
        FromContemplantMessage::ProofStatusResponse(request_id, maybe_proof_status) => {
            // TODO:
        }
        FromContemplantMessage::Heartbeat => {
            // TODO:
        }
    }

    Ok(())
}
