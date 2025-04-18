use crate::artifact_store::ArtifactUri;
use crate::hierophant_state::HierophantState;
use crate::proof_router::WorkerState;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use log::{error, info};
use network_lib::{REGISTER_CONTEMPLANT_ENDPOINT, WorkerRegisterInfo};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};

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

    // .layer(axum::extract::connect_info::IntoConnectInfo::<SocketAddr>::layer())
}
// #[derive(Clone, Debug)]
// struct MyConnectionInfo {
//     ip: String,
// }
//
// impl Connected<IncomingStream<'_>> for MyConnectionInfo {
//     fn connect_info(target: IncomingStream<'_>) -> Self {
//         MyConnectionInfo {
//             ip: target.remote_addr().to_string(),
//         }
//     }
// }

async fn handle_register_worker(
    State(state): State<Arc<HierophantState>>,
    // ConnectInfo(addr): ConnectInfo<MyConnectionInfo>,
    Json(worker_register_info): Json<WorkerRegisterInfo>,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Worker Registration Request ===");

    //println!("my connection info: {:?}", addr);

    info!(
        "Received worker ready check from {:?}",
        worker_register_info
    );

    let worker_addr = format!(
        "http://{}:{}",
        worker_register_info.ip, worker_register_info.port
    );

    match state
        .proof_router
        .worker_registry_client
        .worker_ready(worker_addr.clone())
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

// Client requests to download an artifact (client only ever downloads proofs)
async fn handle_artifact_download(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<ArtifactUri>,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Download Request ===");
    info!("Artifact uri {uri}");

    match state
        .artifact_store_client
        .get_artifact_bytes(uri.clone())
        .await
    {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => {
            let error_msg = format!("Artifact {uri} not found");
            error!("{error_msg}");
            Err(StatusCode::BAD_REQUEST)
        }
        Err(e) => {
            error!("{e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

// Handler for artifact uploads
async fn handle_artifact_upload(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<ArtifactUri>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Upload Request ===");
    info!("Artifact uri {uri}");

    info!("Received artifact data: {} bytes", body.len());

    match state.artifact_store_client.save_artifact(uri, body).await {
        Ok(_) => Ok("Upload successful"),
        Err(e) => {
            error!("{e}");
            Err(StatusCode::BAD_REQUEST)
        }
    }
}
