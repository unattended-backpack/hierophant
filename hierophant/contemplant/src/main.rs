mod config;
mod types;

use alloy_primitives::{B256, hex};
use tokio::time::Instant;

use crate::config::Config;
use crate::types::{AppError, ProofRequest, ProofStatus, ProofStore};
use anyhow::{Context, Result, anyhow};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
};
use log::{error, info};
use reqwest::Client;
use sp1_sdk::{
    CpuProver, CudaProver, Prover, ProverClient, network::proto::network::ProofMode, utils,
};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::RwLock,
    time::{Duration, sleep},
};
use tower_http::limit::RequestBodyLimitLayer;

const WORKER_REGISTER_ENDPOINT: &str = "worker";

#[derive(Clone)]
pub struct WorkerState {
    config: Config,
    cuda_prover: Arc<CudaProver>,
    mock_prover: Arc<CpuProver>,
    proof_store: Arc<ProofStore>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Enable logging.
    // unsafe {
    //     env::set_var("RUST_LOG", "info");
    // }

    let config = tokio::fs::read_to_string("contemplant.toml")
        .await
        .context("read contemplant.toml file")?;

    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Set up the SP1 SDK logger.
    utils::setup_logger();

    // TODO: test if we can have both of these initialized
    let cuda_prover = Arc::new(ProverClient::builder().cuda().build());
    let mock_prover = Arc::new(ProverClient::builder().mock().build());

    let proof_store = Arc::new(RwLock::new(HashMap::new()));

    let worker_state = WorkerState {
        cuda_prover,
        mock_prover,
        config: config.clone(),
        proof_store: proof_store.clone(),
    };

    let app = Router::new()
        .route("/request_proof", post(request_proof))
        .route("/status/:proof_id", get(get_proof_status))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(102400 * 1024 * 1024))
        .with_state(worker_state);

    let port = config.port.to_string();
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    let local_addr = listener.local_addr().unwrap();

    // Send a "ready" notification to the hierophant
    register_worker(config.clone());

    info!("Worker server listening on {}", local_addr);
    axum::serve(listener, app).await?;
    Ok(())
}

fn register_worker(config: Config) {
    tokio::spawn(async move {
        // Give this server a moment to fully initialize
        sleep(Duration::from_secs(1)).await;

        let client = Client::new();

        // Attempt to register with the hierophant
        match client
            .put(format!(
                "{}/{WORKER_REGISTER_ENDPOINT}",
                config.hierophant_address
            ))
            // TODO: what should we send to the hierophant to verify this worker?
            .json(&config)
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    info!(
                        "Successfully registered with hierophant at {}",
                        config.hierophant_address
                    );
                } else {
                    error!(
                        "Failed to register with hierophant {}: HTTP {}",
                        config.hierophant_address,
                        response.status()
                    );
                }
            }
            Err(err) => {
                error!("Failed to connect to hierophant: {}", err);
            }
        }
    });
}

// uses the CudaProver to execute proofs given the elf, ProofMode, and SP1Stdin
// provided by the Hierophant
async fn request_proof(
    State(state): State<WorkerState>,
    Json(payload): Json<ProofRequest>,
) -> Result<StatusCode, AppError> {
    info!("Received proof request {payload}");

    // proof starts as unexecuted
    let initial_status = ProofStatus::unexecuted();

    // It is assumed that the Hierophant won't request the same proof twice
    state
        .proof_store
        .write()
        .await
        .insert(payload.proof_id, initial_status);

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

        // Turn proof struct into bytes
        let proof_bytes_res = proof_res.and_then(|proof| {
            if let ProofMode::Compressed = payload.mode {
                // If it's a compressed proof, we need to serialize the entire struct with bincode.
                // Note: We're re-serializing the entire struct with bincode here, but this is fine
                // because we're on localhost and the size of the struct is small.
                bincode::serialize(&proof)
                    .map_err(|e| anyhow!("Error serializing compressed proof {e}"))
            } else {
                // TODO: It's unclear if we can do this for ProofMode::Core
                Ok(proof.bytes())
            }
        });

        // Create new proof status based on success or error
        let updated_proof_status = match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed proof {} in {} minutes", payload, minutes);
                ProofStatus::executed(proof_bytes)
            }
            Err(e) => {
                error!("Error proving {} at minute {}: {e}", payload, minutes);
                ProofStatus::unexecutable()
            }
        };

        // record new proof status, overwriting initial status of `unexecuted`
        state
            .proof_store
            .write()
            .await
            .insert(payload.proof_id, updated_proof_status);
    });

    Ok(StatusCode::OK)
}

async fn get_proof_status(
    State(state): State<WorkerState>,
    Path(proof_id): Path<String>,
) -> Result<(StatusCode, Json<ProofStatus>), AppError> {
    let proof_id_bytes = hex::decode(&proof_id)?;
    let proof_id = B256::from_slice(&proof_id_bytes);
    info!("Received proof status request: {:?}", proof_id);

    let proof_store = state.proof_store.read().await;

    let proof_status: ProofStatus = match proof_store.get(&proof_id) {
        Some(status) => {
            info!("Proof status of {proof_id}: {}", status);
            status.clone()
        }
        None => {
            error!("Proof {} not found", proof_id);
            ProofStatus::unexecutable()
        }
    };

    Ok((StatusCode::OK, Json(proof_status)))
}
