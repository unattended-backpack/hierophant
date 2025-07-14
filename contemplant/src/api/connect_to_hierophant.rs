use crate::{
    config::Config, message_handler::handle_message_from_hierophant, worker_state::WorkerState,
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, trace, warn};
use network_lib::{
    WorkerRegisterInfo, messages::FromContemplantMessage, protocol::CONTEMPLANT_VERSION,
};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::protocol::{Message, WebSocketConfig},
};

// Starts processes to connect to and initialize with Hierophant
//
// Order of operations:
//  - initiates a ws with the Heirophant.
//  - send Hierophant register message, making the Hierophant aware of this Contemplant
//  - start task that receives messages from Hierophant
//  - start task that sends a Heartbeat messages to Hierophant
//  - if all the above is successful and Hierophant is aware of this Contemplant,
//    send register message to the Magister (if this Contemplant has a Magister)
pub async fn connect_to_hierophant(config: Config, worker_state: WorkerState) -> Result<()> {
    let hierophant_ws_address = config.hierophant_ws_address.clone();

    let ws_config = Some(WebSocketConfig {
        // disable max size limits.  Proofs are large
        max_message_size: None,
        max_frame_size: None,
        ..WebSocketConfig::default()
    });

    let ws_stream = match connect_async_with_config(hierophant_ws_address, ws_config, false).await {
        Ok((stream, response)) => {
            info!("Handshake to Hierophant has been completed");
            // This will be the HTTP response, same as with server this is the last moment we
            // can still access HTTP stuff.
            info!("Hierophant response was {response:?}");
            stream
        }
        Err(e) => {
            let error_msg = format!("WebSocket handshake failed with {e}!");
            error!("{error_msg}");
            return Err(anyhow!(error_msg));
        }
    };

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let worker_register_info = WorkerRegisterInfo {
        contemplant_version: CONTEMPLANT_VERSION.into(),
        name: config.contemplant_name.clone(),
        magister_drop_endpoint: config.magister_drop_endpoint.clone(),
    };

    info!(
        "Sending hierophant at {} worker_register_info {:?}",
        config.hierophant_ws_address, worker_register_info
    );

    let register_message =
        bincode::serialize(&FromContemplantMessage::Register(worker_register_info))
            .context("Serialize worker_register_info")?;

    // send a register request to hierophant
    ws_sender
        .send(Message::Binary(register_message))
        .await
        .context("Send contemplant register info to hierophant")?;

    // Spawns a task that waits for an "exit" message from any thread currently proving.
    // When a thread hits an error in `cuda_prover.prove(...)` it will send a message
    // here to gracefully seppuku.
    let (exit_sender, mut exit_receiver): (mpsc::Sender<String>, mpsc::Receiver<String>) =
        mpsc::channel(10);
    let mut exit_task = tokio::spawn(async move {
        if let Some(error_msg) = exit_receiver.recv().await {
            error!("{error_msg}");
        }
    });

    // Channel for inter-contemplant use.  Sends messages from the thread that receives tasks from the
    // hierophant ws conntection to the thread that sends ws messages back to the hierophant
    let (response_sender, mut response_receiver) = mpsc::channel(100);

    // this thread solely sends ws messages back to the hierophant
    let mut send_task = tokio::spawn(async move {
        while let Some(ws_msg) = response_receiver.recv().await {
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

            // send the message to the Hierophant
            let msg = Message::Binary(ws_msg_bytes);
            if let Err(e) = ws_sender.send(msg).await {
                error!("Error sending message to hierophant: {e}");
                break;
            }
        }

        // close connection cleanly when contemplant is done
        if let Err(e) = ws_sender.send(Message::Close(None)).await {
            warn!("Could not send Close due to {e:?}, probably it is ok?");
        };
    });

    // this thread receives commands from the Hierophant, processes them, and
    // sometimes sends responses back to Hierophant using the response_sender (which
    // sends messages to send_task)
    let response_sender_clone = response_sender.clone();
    let worker_state_clone = worker_state.clone();
    let mut recv_task = tokio::spawn(async move {
        let worker_state = worker_state_clone;
        while let Some(msg_result) = ws_receiver.next().await {
            trace!("Got ws message from hierophant");
            match msg_result {
                Ok(msg) => {
                    // got some message from hierophant
                    if handle_message_from_hierophant(
                        worker_state.clone(),
                        msg,
                        response_sender_clone.clone(),
                        exit_sender.clone(),
                    )
                    .await
                    .is_break()
                    {
                        warn!("Received break message");
                        break;
                    }
                }
                Err(e) => {
                    error!("Error receiving message from hierophant: {e}");
                    break;
                }
            }
        }
    });

    // Spawns a task that sends a Heartbeat message every <heartbeat_interval_seconds> to the
    // Hierophant
    let response_sender_clone = response_sender.clone();
    let mut heartbeat_task = tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(config.heartbeat_interval_seconds));
        loop {
            interval.tick().await;
            let heartbeat = FromContemplantMessage::Heartbeat;
            if response_sender_clone.send(heartbeat).await.is_err() {
                // Channel closed, exit
                break;
            }
        }
    });

    if let Some(drop_endpoint) = &config.magister_drop_endpoint {
        info!(
            "Contemplant is being managed by the Magister with drop endpoint {drop_endpoint}."
        );
        verify_with_magister(drop_endpoint.clone()).await?;
    }

    // contemplant is now ready for requests.  Change ready to true.
    *(worker_state.ready.lock().await) = true;

    //wait for either task to finish and kill the other task
    tokio::select! {
        _ = (&mut send_task) => {
            info!("send task exited");
            recv_task.abort();
            heartbeat_task.abort();
            exit_task.abort();
        },
        _ = (&mut recv_task) => {
            info!("recv task exited");
            send_task.abort();
            heartbeat_task.abort();
            exit_task.abort();
        }
        _ = (&mut heartbeat_task) => {
            info!("heartbeat task exited");
            recv_task.abort();
            send_task.abort();
            exit_task.abort();
        }
        _ = (&mut exit_task) => {
            info!("Got an error from proving thread.  Exiting");
            recv_task.abort();
            send_task.abort();
            heartbeat_task.abort();
        }
    }

    Ok(())
}

async fn verify_with_magister(drop_endpoint: String) -> Result<()> {
    let url = drop_endpoint.replace("drop", "verify");

    let resp = match reqwest::Client::new().get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            let err = format!("Send verify request to Magister at {url}: {e}");
            error!("{err}");
            return Err(anyhow!("{err}"));
        }
    };

    match resp.error_for_status() {
        Ok(_) => Ok(()),
        Err(e) => {
            let err = format!("Receive verify response from Magister at {url}: {e}");
            error!("{err}");
            Err(anyhow!("{err}"))
        }
    }
}
