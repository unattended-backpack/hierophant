mod assessor;
mod config;
mod types;

use alloy_primitives::B256;
use assessor::start_assessor;
use config::AssessorConfig;
use futures_util::{SinkExt, StreamExt};
use network_lib::ProofFromNetwork;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::protocol::{Message, WebSocketConfig},
};
use types::ProofStoreClient;

use crate::config::Config;
use crate::types::ProofStore;
use anyhow::{Context, Result, anyhow};
use log::{error, info, trace, warn};
use network_lib::{
    CONTEMPLANT_VERSION, ContemplantProofRequest, ContemplantProofStatus, FromContemplantMessage,
    FromHierophantMessage, WorkerRegisterInfo,
};
use sp1_sdk::{
    CpuProver, CudaProver, Prover, ProverClient,
    network::proto::network::{ExecutionStatus, ProofMode},
    utils,
};
use std::{ops::ControlFlow, sync::Arc};
use tokio::time::Duration;

#[derive(Clone)]
pub struct WorkerState {
    // config: Config,
    cuda_prover: Arc<CudaProver>,
    mock_prover: Arc<CpuProver>,
    proof_store_client: ProofStoreClient,
    assessor_config: AssessorConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = tokio::fs::read_to_string("contemplant.toml")
        .await
        .context("read contemplant.toml file")?;
    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Set up the SP1 SDK logger.
    utils::setup_logger();
    info!("Starting contemplant {}", config.contemplant_name);

    // compiler will always complain about one of these branches being unreachable, depending on if
    // you compiled with `features enable-native-gnark` or not
    let cuda_prover = match &config.moongate_endpoint {
        // build with undockerized moongate server
        Some(moongate_endpoint) => {
            // make sure the `native-gnark` feature is enabled.  Otherwise the contemplant will
            // error when it tries to finish a GROTH16 proof
            #[cfg(not(feature = "enable-native-gnark"))]
            {
                let error_msg = "Please rebuild with: `--features enable-native-gnark` or remove moongate_endpoint in cargo.toml to use the dockerized CUDA prover";
                error!("{error_msg}");
                panic!("{error_msg}");
            }
            info!("Building CudaProver with moongate endpoint {moongate_endpoint}...");
            Arc::new(
                ProverClient::builder()
                    .cuda()
                    .with_moongate_endpoint(moongate_endpoint)
                    .build(),
            )
        }
        // spin up cuda prover docker container
        None => {
            // build using dockerized CUDA prover
            #[cfg(feature = "enable-native-gnark")]
            {
                let error_msg = "Please rebuild without `--features enable-native-gnark` to use the dockerized CUDA prover or supply a moongate_endpoint to contemplant.toml";
                error!("{error_msg}");
                panic!("{error_msg}");
            }
            info!("Starting CudaProver docker container...");
            Arc::new(ProverClient::builder().cuda().build())
        }
    };
    let mock_prover = Arc::new(ProverClient::builder().mock().build());
    info!("Prover built");

    let proof_store_client = ProofStore::new(config.max_proofs_stored);

    let worker_state = WorkerState {
        cuda_prover,
        mock_prover,
        proof_store_client,
        assessor_config: config.assessor,
    };

    let ws_config = Some(WebSocketConfig {
        max_message_size: None,
        max_frame_size: None,
        ..WebSocketConfig::default()
    });
    let hierophant_ws_address = config.hierophant_ws_address.clone();
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
        while let Some(error_msg) = exit_receiver.recv().await {
            error!("{error_msg}");
            break;
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
                    let error_msg = format!("Error serializing message {}: {e}", ws_msg);
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
    let mut recv_task = tokio::spawn(async move {
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

    info!("Shutting down");
    Ok(())
}

async fn handle_message_from_hierophant(
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
            error!("Unsupported message type {:?}", msg);
            return ControlFlow::Continue(());
        }
    };

    trace!("Handling FromHierophantMessage {msg}");

    match msg {
        FromHierophantMessage::ProofRequest(proof_request) => {
            request_proof(state, proof_request, exit_sender).await
        }
        FromHierophantMessage::ProofStatusRequest(request_id) => {
            let start = tokio::time::Instant::now();
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
            let elapsed = start.elapsed().as_secs_f64();
            info!("took {elapsed} seconds to get and return proof request");
        }
    };

    ControlFlow::Continue(())
}

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode, and SP1Stdin
// provided by the Hierophant
async fn request_proof(
    state: WorkerState,
    proof_request: ContemplantProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
    info!("Received proof request {proof_request}");

    // proof starts as unexecuted
    let initial_status = ContemplantProofStatus::unexecuted();

    // It is assumed that the Hierophant won't request the same proof twice
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    let (assessor_shutdown_tx, assessor_shutdown_rx) = watch::channel(false);
    if let Err(e) = start_assessor(
        state.mock_prover.clone(),
        &proof_request.elf,
        &proof_request.sp1_stdin,
        state.assessor_config,
        assessor_shutdown_rx,
        state.proof_store_client.clone(),
        proof_request.request_id,
    )
    .await
    {
        error!("Assessor error: {e}");
    };

    tokio::spawn(async move {
        let start_time = Instant::now();
        let mock = proof_request.mock;
        let stdin = &proof_request.sp1_stdin;

        let (pk, _) = if mock {
            state.mock_prover.setup(&proof_request.elf)
        } else {
            // the cuda prover keeps state of the last `setup()` that was called on it.
            // You must call `setup()` then `prove` *each* time you intend to
            // prove a certain program
            state.cuda_prover.setup(&proof_request.elf)
        };

        // construct proving function based on ProofMode and if it's a CUDA or mock proof
        let proof_res = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => Err(anyhow!("UnspecifiedProofMode")),
            ProofMode::Core => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).core().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).core().run()
                }
            }
            ProofMode::Compressed => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).compressed().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).compressed().run()
                }
            }
            ProofMode::Plonk => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).plonk().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).plonk().run()
                }
            }
            ProofMode::Groth16 => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).groth16().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).groth16().run()
                }
            }
        };

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        let proof_bytes_res = proof_res.and_then(|proof| {
            let network_proof: ProofFromNetwork = proof.into();
            bincode::serialize(&network_proof).map_err(|e| anyhow!("Error serializing proof {e}"))
        });

        // Update new proof status based on success or error
        match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed proof {} in {} minutes", proof_request, minutes);
                state
                    .proof_store_client
                    .proof_status_update(
                        proof_request.request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
                    )
                    .await;
            }
            Err(e) => {
                let error_msg =
                    format!("Error proving {} at minute {}: {e}", proof_request, minutes);

                // If a contemplant errors while making a proof it should seppuku.
                // This message will force the program to exit gracefully.
                exit_sender
                    .send(error_msg)
                    .await
                    .context("Send exit error message to main thread")
                    .unwrap();

                state
                    .proof_store_client
                    .proof_status_update(
                        proof_request.request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;
            }
        };

        // send message to stop assessor
        if let Err(err) = assessor_shutdown_tx.send(true) {
            error!("Error sending shutdown signal to assessor: {err}");
        }
    });
}

async fn get_proof_request_status(
    state: WorkerState,
    request_id: B256,
) -> Option<ContemplantProofStatus> {
    let start = tokio::time::Instant::now();
    let status = match state.proof_store_client.get(request_id).await {
        Some(proof_status) => {
            info!(
                "Received proof status request: {:?}.  Proof is {}",
                request_id, proof_status.progress
            );
            Some(proof_status)
        }
        None => {
            warn!(
                "Received proof status request: {:?} but this proof cannot be found",
                request_id
            );
            None
        }
    };
    let secs = start.elapsed().as_secs_f64();

    if secs > 1.0 {
        info!(
            "Slow execution detected: took {} seconds to get proof status for proof request {}",
            secs, request_id
        );
    }

    status
}
