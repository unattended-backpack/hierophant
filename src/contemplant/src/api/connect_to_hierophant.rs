use crate::{
    config::Config, message_handler::handle_message_from_hierophant, worker_state::WorkerState,
};

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use log::{error, info, trace, warn};
use network_lib::{
    WorkerRegisterInfo, messages::FromContemplantMessage, protocol::CONTEMPLANT_VERSION,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
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
pub async fn connect_to_hierophant(
    config: Config,
    worker_state: WorkerState,
    response_sender: mpsc::Sender<FromContemplantMessage>,
    response_receiver: Arc<tokio::sync::Mutex<mpsc::Receiver<FromContemplantMessage>>>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
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
        supported_vms: worker_state.supported_vms(),
        groth16_enabled: worker_state.groth16_enabled(),
        openvm_evm_enabled: worker_state.openvm_evm_enabled(),
        magister_drop_endpoint: config.magister_drop_endpoint.clone(),
        instance_nonce: worker_state.instance_nonce,
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

    // The response channel is PERSISTENT (owned by main, shared across
    // reconnects) so proving tasks spawned under one connection can
    // deliver results over whichever connection is live when they
    // finish. Heartbeats ride a per-connection channel that send_task
    // drains with priority, so a queue of large proof uploads can
    // never starve liveness signaling.
    let (heartbeat_sender, mut heartbeat_receiver) =
        mpsc::channel::<FromContemplantMessage>(8);

    // this thread solely sends ws messages back to the hierophant.
    // Biased select: pending heartbeats always jump the response
    // queue; a shutdown broadcast produces a clean Close frame.
    let response_receiver_clone = response_receiver.clone();
    let mut send_shutdown_rx = shutdown_rx.resubscribe();
    let mut send_task = tokio::spawn(async move {
        // Holding the lock for the lifetime of this connection is
        // correct: exactly one connection is live at a time, and an
        // aborted or exited send_task releases it for the successor.
        let mut response_receiver = response_receiver_clone.lock().await;
        loop {
            let ws_msg = tokio::select! {
                biased;
                _ = send_shutdown_rx.recv() => break,
                maybe_hb = heartbeat_receiver.recv() => match maybe_hb {
                    Some(hb) => hb,
                    None => break,
                },
                maybe_msg = response_receiver.recv() => match maybe_msg {
                    Some(msg) => msg,
                    None => break,
                },
            };
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
    // Hierophant (over the priority channel send_task drains first).
    let hb_interval = Duration::from_secs(config.heartbeat_interval_seconds);
    let mut heartbeat_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(hb_interval);
        // After a stall, one late heartbeat is all hierophant needs;
        // a catch-up burst is just queue noise.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_tick = tokio::time::Instant::now();
        loop {
            interval.tick().await;
            // Self-diagnose scheduling starvation: a timer firing far
            // past its interval means this runtime (or the whole
            // process) was starved of CPU — classically by proving
            // work saturating every core. Signal to lower
            // RAYON_NUM_THREADS / reserve cores.
            let late_by = last_tick.elapsed().saturating_sub(hb_interval);
            if late_by > hb_interval {
                warn!(
                    "Heartbeat timer fired {}s late (interval {}s) — runtime/CPU \
                     starvation; check proving thread counts",
                    late_by.as_secs(),
                    hb_interval.as_secs()
                );
            }
            last_tick = tokio::time::Instant::now();
            if heartbeat_sender
                .send(FromContemplantMessage::Heartbeat)
                .await
                .is_err()
            {
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
        // A capability change (e.g. the SP1 CUDA backend permanently
        // demoted after repeated deaths) only reaches the hierophant at
        // registration time, so drop this connection deliberately: the
        // outer loop re-dials within seconds and the fresh
        // WorkerRegisterInfo advertises the reduced capability set.
        _ = worker_state.reconnect.notified() => {
            info!("Capability change requested re-registration; dropping ws to reconnect");
            recv_task.abort();
            send_task.abort();
            heartbeat_task.abort();
            exit_task.abort();
        }
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
        _ = shutdown_rx.recv() => {
            // SIGINT/SIGTERM broadcast from main; perform a clean WebSocket
            // close instead of letting docker SIGKILL tear the TCP connection
            // down mid-frame. send_task observes the same shutdown
            // broadcast through its own subscription and emits
            // `Message::Close(None)` before exiting, which hierophant
            // logs at INFO instead of the alarming "Connection reset
            // without closing handshake" ERROR.
            info!("Shutdown signal received; closing hierophant WebSocket cleanly");
            recv_task.abort();
            heartbeat_task.abort();
            exit_task.abort();
            // send_task listens on its own shutdown subscription and
            // emits `Message::Close(None)` before exiting; the
            // response channel itself is persistent (owned by main)
            // and intentionally stays open across reconnects.
            // Bounded wait; if hierophant is already gone the sendpath may
            // fail; either way we shouldn't block shutdown for longer than
            // docker's default grace period.
            match tokio::time::timeout(Duration::from_secs(3), &mut send_task).await {
                Ok(_) => info!("send task finished after clean close"),
                Err(_) => {
                    warn!("send task didn't finish within 3s; aborting");
                    send_task.abort();
                }
            }
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
