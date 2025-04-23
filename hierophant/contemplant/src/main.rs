mod config;
mod types;

use alloy_primitives::B256;
use futures_util::stream::FuturesUnordered;
use futures_util::{SinkExt, StreamExt};
use tokio::{sync::mpsc, time::Instant};
use types::ProofFromNetwork;

use tokio_tungstenite::{
    connect_async,
    tungstenite::protocol::{CloseFrame, Message, frame::coding::CloseCode},
};

use crate::config::Config;
use crate::types::{AppError, ProofStore};
use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
};
use log::{error, info};
use network_lib::{
    CONTEMPLANT_VERSION, ContemplantProofRequest, ContemplantProofStatus, FromContemplantMessage,
    FromHierophantMessage, REGISTER_CONTEMPLANT_ENDPOINT, WorkerRegisterInfo,
};
use reqwest::Client;
use sp1_sdk::{
    CpuProver, CudaProver, Prover, ProverClient, network::proto::network::ProofMode, utils,
};
use std::str::FromStr;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

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
            println!("Could not send Close due to {e:?}, probably it is ok?");
        };
    });

    // this thread receives commands from the Hierophant, processes them, and
    // sometimes sends responses back to Hierophant using the response_sender (which
    // sends messages to send_task)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            // got some message from hierophant
            if let Err(e) = handle_message_from_hierophant(msg, response_sender.clone()) {
                error!("Error handling message {e}");
                break;
            }
        }
    });

    //wait for either task to finish and kill the other task
    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
        },
        _ = (&mut recv_task) => {
            send_task.abort();
        }
    }

    Ok(())
}

fn handle_message_from_hierophant(
    msg: Message,
    response_sender: mpsc::Sender<FromContemplantMessage>,
) -> Result<()> {
    // parse message into FromHierophantMessage
    let msg: FromHierophantMessage = match msg {
        Message::Binary(bytes) => match bincode::deserialize(&bytes) {
            Ok(ws_msg) => ws_msg,
            Err(e) => {
                error!("Error deserializing message");
                return Ok(());
            }
        },
        _ => {
            error!("Unsupported message type {:?}", msg);
            return Ok(());
        }
    };

    match msg {
        FromHierophantMessage::ProofRequest(ContemplantProofRequest {
            request_id,
            elf,
            mock,
            mode,
            sp1_stdin,
        }) => {
            // TODO:
        }
        FromHierophantMessage::ProofStatusRequest(proof_request_id) => {
            // TODO:
        }
        FromHierophantMessage::Heartbeat => {
            // TODO:
        }
    }

    Ok(())
}

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode, and SP1Stdin
// provided by the Hierophant
async fn request_proof(
    State(state): State<WorkerState>,
    Json(payload): Json<ContemplantProofRequest>,
) -> Result<StatusCode, AppError> {
    info!("Received proof request {payload}");

    // proof starts as unexecuted
    let initial_status = ContemplantProofStatus::unexecuted();

    // It is assumed that the Hierophant won't request the same proof twice
    state
        .proof_store
        .write()
        .await
        .insert(payload.request_id, initial_status);

    tokio::spawn(async move {
        let start_time = Instant::now();

        let mock = payload.mock;
        let stdin = &payload.sp1_stdin;

        let (pk, _) = if mock {
            state.mock_prover.setup(&payload.elf)
        } else {
            // the cuda prover keeps state of the last `setup()` that was called on it.
            // You must call `setup()` then `prove` *each* time you intend to
            // prove a certain program
            state.cuda_prover.setup(&payload.elf)
        };

        // construct proving function based on ProofMode and if it's a CUDA or mock proof
        let proof_res = match payload.mode {
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
                info!("Completed proof {} in {} minutes", payload, minutes);
                ContemplantProofStatus::executed(proof_bytes)
            }
            Err(e) => {
                error!("Error proving {} at minute {}: {e}", payload, minutes);
                ContemplantProofStatus::unexecutable()
            }
        };

        // record new proof status, overwriting initial status of `unexecuted`
        state
            .proof_store
            .write()
            .await
            .insert(payload.request_id, updated_proof_status);
    });

    Ok(StatusCode::OK)
}

async fn get_proof_request_status(
    State(state): State<WorkerState>,
    Path(request_id): Path<String>,
) -> Result<Json<ContemplantProofStatus>, AppError> {
    let request_id = match B256::from_str(&request_id) {
        Ok(r) => r,
        Err(e) => {
            let error_msg = format!(
                "Couldn't parse request_id {request_id} as B256 in get_proof_request_status. Error {e}"
            );
            error!("{error_msg}");
            return Err(anyhow!("{error_msg}").into());
        }
    };

    info!("Received proof status request: {:?}", request_id);

    let proof_store = state.proof_store.read().await;

    let proof_status: ContemplantProofStatus = match proof_store.get(&request_id) {
        Some(status) => {
            info!("Proof status of {request_id}: {}", status);
            status.clone()
        }
        None => {
            error!("Proof {} not found", request_id);
            ContemplantProofStatus::unexecutable()
        }
    };

    Ok(Json(proof_status))
}
