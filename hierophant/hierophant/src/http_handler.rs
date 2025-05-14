use crate::hierophant_state::HierophantState;
use crate::proof_router::WorkerState;
use crate::{artifact_store::ArtifactUri, proof_router::CompletedProofInfo};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Path, State, ws::WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    routing::{any, get, post, put},
};
use log::{error, info};
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
        // for contemplant ws connections
        .route("/ws", any(ws_handler))
        // Artifact upload endpoint
        .route("/upload/:uri", post(handle_artifact_upload))
        .route("/upload/:uri", put(handle_artifact_upload))
        // Artifact download endpoint
        .route("/:uri", get(handle_artifact_download))
        // Get all healthy contemplants
        .route("/contemplants", get(contemplants))
        // Get all dead contemplants
        .route("/dead-contemplants", get(dead_contemplants))
        // get a history of all completed proofs and the contemplant who finished it
        .route("/proof-history", get(handle_proof_history))
        .with_state(state)
}

// The handler for the HTTP request (this gets called when the HTTP request lands at the start
// of websocket negotiation). After this completes, the actual switching from HTTP to
// websocket protocol will occur.
// This is the last point where we can extract TCP/IP metadata such as IP address of the client
// as well as things from HTTP headers such as user-agent of the browser etc.
async fn ws_handler(
    State(state): State<Arc<HierophantState>>,
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    info!("Received ws connection request from {addr}");

    let one_hundred_mb: usize = 100 * 1024 * 1024; // 100MB

    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    ws.max_message_size(one_hundred_mb)
        .max_frame_size(one_hundred_mb)
        .on_upgrade(move |socket| {
            crate::ws_handler::handle_socket(
                socket,
                addr,
                state.proof_router.worker_registry_client.clone(),
            )
        })
}

async fn dead_contemplants(
    State(state): State<Arc<HierophantState>>,
) -> Result<Json<Vec<(String, WorkerState)>>, StatusCode> {
    match state
        .proof_router
        .worker_registry_client
        .dead_workers()
        .await
    {
        Ok(workers) => Ok(Json(workers)),
        Err(e) => {
            let error_msg = format!("Error sending dead_workers command: {e}");
            error!("{error_msg}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn contemplants(
    State(state): State<Arc<HierophantState>>,
) -> Result<Json<Vec<(String, WorkerState)>>, StatusCode> {
    match state.proof_router.worker_registry_client.workers().await {
        Ok(workers) => Ok(Json(workers)),
        Err(e) => {
            let error_msg = format!("Error sending workers command: {e}");
            error!("{error_msg}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn handle_proof_history(
    State(state): State<Arc<HierophantState>>,
) -> Result<Json<Vec<CompletedProofInfo>>, StatusCode> {
    match state
        .proof_router
        .worker_registry_client
        .proof_history()
        .await
    {
        Ok(completed_proofs) => Ok(Json(completed_proofs)),
        Err(e) => {
            let error_msg = format!("Error sending completed_proofs command: {e}");
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
