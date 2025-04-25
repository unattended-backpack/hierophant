mod config;
mod types;

use alloy_primitives::B256;
use futures_util::{SinkExt, StreamExt};
use tokio::{sync::mpsc, time::Instant};
use types::ProofFromNetwork;

use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::config::Config;
use crate::types::ProofStore;
use anyhow::{Context, Result, anyhow};
use log::{error, info, warn};
use network_lib::{
    CONTEMPLANT_VERSION, ContemplantProofRequest, ContemplantProofStatus, FromContemplantMessage,
    FromHierophantMessage, WorkerRegisterInfo,
};
use sp1_sdk::{
    CpuProver, CudaProver, Prover, ProverClient, network::proto::network::ProofMode, utils,
};
use std::{collections::HashMap, sync::Arc};
use tokio::{sync::RwLock, time::Duration};

#[derive(Clone)]
pub struct WorkerState {
    // config: Config,
    cuda_prover: Arc<CudaProver>,
    mock_prover: Arc<CpuProver>,
    proof_store: Arc<ProofStore>,
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

    let cuda_prover = match &config.moongate_endpoint {
        // build with undockerized moongate server
        Some(moongate_endpoint) => {
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
            info!("Starting CudaProver docker container...");
            Arc::new(ProverClient::builder().cuda().build())
        }
    };
    let mock_prover = Arc::new(ProverClient::builder().mock().build());
    info!("Prover built");

    let proof_store = Arc::new(RwLock::new(HashMap::new()));

    let worker_state = WorkerState {
        cuda_prover,
        mock_prover,
        // config: config.clone(),
        proof_store: proof_store.clone(),
    };

    let ws_stream = match connect_async(config.hierophant_ws_address.clone()).await {
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
        while let Some(Ok(msg)) = ws_receiver.next().await {
            // got some message from hierophant
            if let Err(e) = handle_message_from_hierophant(
                worker_state.clone(),
                msg,
                response_sender_clone.clone(),
            )
            .await
            {
                error!("Error handling message {e}");
                break;
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
            recv_task.abort();
            heartbeat_task.abort();
        },
        _ = (&mut recv_task) => {
            send_task.abort();
            heartbeat_task.abort();
        }
        _ = (&mut heartbeat_task) => {
            recv_task.abort();
            send_task.abort();
        }
    }

    Ok(())
}

async fn handle_message_from_hierophant(
    state: WorkerState,
    msg: Message,
    response_sender: mpsc::Sender<FromContemplantMessage>,
) -> Result<()> {
    // parse message into FromHierophantMessage
    let msg: FromHierophantMessage = match msg {
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

    match msg {
        FromHierophantMessage::ProofRequest(proof_request) => {
            request_proof(state, proof_request).await
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
                return Err(anyhow!(e));
            }
        }
    };

    Ok(())
}

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode, and SP1Stdin
// provided by the Hierophant
async fn request_proof(state: WorkerState, proof_request: ContemplantProofRequest) {
    info!("Received proof request {proof_request}");

    // proof starts as unexecuted
    let initial_status = ContemplantProofStatus::unexecuted();

    // It is assumed that the Hierophant won't request the same proof twice
    state
        .proof_store
        .write()
        .await
        .insert(proof_request.request_id, initial_status);

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

        // Create new proof status based on success or error
        let updated_proof_status = match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed proof {} in {} minutes", proof_request, minutes);
                ContemplantProofStatus::executed(proof_bytes)
            }
            Err(e) => {
                error!("Error proving {} at minute {}: {e}", proof_request, minutes);
                ContemplantProofStatus::unexecutable()
            }
        };

        // record new proof status, overwriting initial status of `unexecuted`
        state
            .proof_store
            .write()
            .await
            .insert(proof_request.request_id, updated_proof_status);
    });
}

async fn get_proof_request_status(
    state: WorkerState,
    request_id: B256,
) -> Option<ContemplantProofStatus> {
    info!("Received proof status request: {:?}", request_id);

    let proof_store = state.proof_store.read().await;

    proof_store.get(&request_id).cloned()
}
