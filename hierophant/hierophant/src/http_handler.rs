use crate::artifact_store::ArtifactUri;
use crate::hierophant_state::HierophantState;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use log::{error, info};
use network_lib::{CONTEMPLANT_VERSION, REGISTER_CONTEMPLANT_ENDPOINT, WorkerRegisterInfo};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::SocketAddr,
    sync::Arc,
};

// Structure to receive worker registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegistration {
    pub name: String,
}

// Create the router with all routes
pub fn create_router(state: Arc<HierophantState>) -> Router {
    Router::new()
        // Worker registration endpoint
        .route(
            format!("/{REGISTER_CONTEMPLANT_ENDPOINT}").as_ref(),
            put(handle_register_worker),
        )
        .route(
            format!("/{REGISTER_CONTEMPLANT_ENDPOINT}").as_ref(),
            post(handle_register_worker),
        )
        // Artifact upload endpoint
        .route("/upload/:uri", post(handle_artifact_upload))
        .route("/upload/:uri", put(handle_artifact_upload))
        // Artifact download endpoint
        .route("/:uri", get(handle_artifact_download))
        // Get all healthy contemplants
        .route("/contemplants", get(contemplants))
        .with_state(state)
}

async fn handle_register_worker(
    State(state): State<Arc<HierophantState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(worker_register_info): Json<WorkerRegisterInfo>,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Worker Registration Request ===");

    let worker_addr = format!("http://{}:{}", addr.ip(), worker_register_info.port);

    info!(
        "Received contemplant ready check from {} at {}",
        worker_register_info.name, worker_addr
    );

    // check contemplant version
    if CONTEMPLANT_VERSION != worker_register_info.contemplant_version {
        error!(
            "Contemplant {} at {} running incorrect CONTEMPLANT_VERSION: {}. This Hierophant's CONTEMPLANT_VERSION: {}",
            worker_register_info.name,
            worker_addr,
            worker_register_info.contemplant_version,
            CONTEMPLANT_VERSION
        );
        return Err(StatusCode::UPGRADE_REQUIRED);
    } else {
        match state
            .proof_router
            .worker_registry_client
            .worker_ready(worker_addr.clone(), worker_register_info.name)
            .await
        {
            Ok(_) => Ok(StatusCode::OK),
            Err(e) => {
                let error_msg =
                    format!("Error sending worker_ready command for worker {worker_addr}: {e}");
                error!("{error_msg}");
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }
}

async fn contemplants(
    State(state): State<Arc<HierophantState>>,
) -> Result<Json<Vec<String>>, StatusCode> {
    match state.proof_router.worker_registry_client.workers().await {
        Ok(workers) => {
            let workers: Vec<String> = workers
                .iter()
                .map(|(addr, state)| format!("addr: {addr}, {state}"))
                .collect();

            Ok(Json(workers))
        }
        //Ok(Json(workers)),
        Err(e) => {
            let error_msg = format!("Error sending workers command: {e}");
            error!("{error_msg}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// Client requests to download an artifact (client only ever downloads proofs)
async fn handle_artifact_download(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<ArtifactUri>,
) -> Result<Vec<u8>, StatusCode> {
    info!("\n=== Received Download Request ===");

    let bytes = match state
        .artifact_store_client
        .get_artifact_bytes(uri.clone())
        .await
    {
        Ok(Some(bytes)) => {
            info!(
                "Client downloading artifact {uri} with {} bytes.  Artifact bytes as hex: {}",
                bytes.len(),
                display_artifact_hex(&bytes)
            );
            bytes
        }
        Ok(None) => {
            let error_msg = format!("Artifact {uri} not found");
            error!("{error_msg}");
            return Err(StatusCode::BAD_REQUEST);
        }
        Err(e) => {
            error!("Error downloading artifact {uri}: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    Ok(bytes)
}

// Handler for artifact uploads
async fn handle_artifact_upload(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<ArtifactUri>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Upload Request ===");

    info!("Received {} bytes of artifact {uri}", body.len());

    match state
        .artifact_store_client
        .save_artifact(uri.clone(), body)
        .await
    {
        Ok(_) => Ok("Upload successful"),
        Err(e) => {
            error!("Error uploading artifact {uri}: {e}");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

fn display_artifact_hex(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let hash = hasher.finish();

    format!("{:016x}", hash)
}
