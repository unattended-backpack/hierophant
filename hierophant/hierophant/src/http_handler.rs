use crate::hierophant_state::{HierophantState, WorkerState, WorkerStatus};
use axum::{
    Json, Router,
    body::Bytes,
    debug_handler,
    extract::{
        Path, State,
        connect_info::{self, ConnectInfo, Connected},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    serve::IncomingStream,
};
use log::info;
use network_lib::WorkerRegisterInfo;
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::net::{UnixListener, UnixStream, unix::UCred};
use uuid::Uuid;

// Structure to receive worker registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegistration {
    pub name: String,
}

// Create the router with all routes
pub fn create_router(state: Arc<HierophantState>) -> Router {
    Router::new()
        // Worker registration endpoint
        //.route("/worker", get(handle_register_worker))
        .route("/register_worker", post(handle_register_worker))
        /*
                // Artifact upload endpoint
                .route("/:id", post(handle_artifact_upload))
                .route("/:id", put(handle_artifact_upload))
                // Artifact download endpoint
                .route("/:id", get(handle_artifact_download))
        */
        // Add more routes as needed
        .with_state(state)

    // .layer(axum::extract::connect_info::IntoConnectInfo::<SocketAddr>::layer())
}
#[derive(Clone, Debug)]
struct MyConnectionInfo {
    ip: String,
}

impl Connected<IncomingStream<'_>> for MyConnectionInfo {
    fn connect_info(target: IncomingStream<'_>) -> Self {
        MyConnectionInfo {
            ip: target.remote_addr().to_string(),
        }
    }
}

async fn handle_register_worker(
    State(state): State<Arc<HierophantState>>,
    ConnectInfo(addr): ConnectInfo<MyConnectionInfo>,
    Json(worker_register_info): Json<WorkerRegisterInfo>,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Worker Registration Request ===");

    println!("my connection info: {:?}", addr);

    println!("Worker register info: {:?}", worker_register_info);
    // TODO: store worker address.
    // let worker_info = WorkerInfo {
    //     id: Uuid::new_v4().to_string(),
    //     name: worker_registration.name,
    //     status: WorkerStatus::Idle,
    // };
    //
    //let worker = WorkerState::new(worker_registration.name, addr);

    // info!("Registering worker:");
    // info!("  Name: {}", worker.name);
    // info!("  ID: {}", worker.id);
    // info!("  Status: {}", worker.status);
    // info!("  Address: {}", addr);

    /*
    // Store the worker info
    state
        .workers
        .write()
        .await
        .insert(worker_info.id.clone(), worker_info);
    */

    // Return success response with the worker ID
    Ok(StatusCode::OK)
}

async fn handle_artifact_download(
    State(state): State<Arc<HierophantState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    // TODO

    let artifact = "todo";
    Ok(Json(artifact))
}

// Handler for artifact uploads
async fn handle_artifact_upload(
    State(state): State<Arc<HierophantState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    let path = format!("/upload/{}", id);

    println!("\n=== Received Upload Request ===");
    println!("Path: {}", path);

    // Check if this is a valid upload URL
    let is_valid = state.upload_urls.lock().await.contains(&path);

    if !is_valid {
        println!("Invalid upload URL: {}", path);
        return Err(StatusCode::NOT_FOUND);
    }

    println!("Received upload data: {} bytes", body.len());

    // Print a preview of the data
    if !body.is_empty() {
        let preview_size = std::cmp::min(100, body.len());
        println!(
            "Data preview (first {} bytes): {}",
            preview_size,
            hex::encode(&body[..preview_size])
        );
    }

    // Return success
    Ok("Upload successful")
}
