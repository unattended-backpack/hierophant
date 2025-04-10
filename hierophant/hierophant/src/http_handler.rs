use crate::hierophant_state::{HierophantState, WorkerInfo, WorkerStatus};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
        .route("/worker", put(register_worker))
        .route("/worker", post(register_worker))
        // Artifact upload endpoint
        .route("/:id", post(handle_artifact_upload))
        .route("/:id", put(handle_artifact_upload))
        .route("/:id", get(handle_artifact_download))
        // Add more routes as needed
        .with_state(state)
}

// Handler for worker registration
async fn register_worker(
    State(state): State<Arc<HierophantState>>,
    Json(worker_registration): Json<WorkerRegistration>,
) -> impl IntoResponse {
    println!("\n=== Received Worker Registration Request ===");

    // Update the last heartbeat
    let last_heartbeat = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // TODO: store worker address.
    let worker_info = WorkerInfo {
        id: Uuid::new_v4().to_string(),
        name: worker_registration.name,
        status: WorkerStatus::Available,
        last_heartbeat,
    };

    println!("Registering worker:");
    println!("  Name: {}", worker_info.name);
    println!("  ID: {}", worker_info.id);
    println!("  Status: {:?}", worker_info.status);

    // Store the worker info
    let mut workers = state.workers.lock().unwrap();
    workers.insert(worker_info.id.clone(), worker_info.clone());

    // Return success response with the worker ID
    (StatusCode::OK, Json(worker_info))
}

async fn handle_artifact_download(
    State(state): State<Arc<HierophantState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    // TODO
    todo!()
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
    let is_valid = state.upload_urls.lock().unwrap().contains(&path);

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
