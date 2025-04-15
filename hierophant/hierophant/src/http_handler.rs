use crate::hierophant_state::{Artifact, HierophantState, WorkerStatus};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, State,
        connect_info::{self, ConnectInfo, Connected},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    serve::IncomingStream,
};
use log::{error, info};
use network_lib::{REGISTER_WORKER_ENDPOINT, WorkerRegisterInfo};
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
        .route(
            format!("/{REGISTER_WORKER_ENDPOINT}").as_ref(),
            put(handle_register_worker),
        )
        .route(
            format!("/{REGISTER_WORKER_ENDPOINT}").as_ref(),
            post(handle_register_worker),
        )
        // for testing only
        //        .route("/test-register-uri", post(handle_test_register_uri))
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

// TODO:
async fn contemplants(
    State(state): State<Arc<HierophantState>>,
) -> Result<Json<Vec<(String, WorkerState)>>, StatusCode> {
    todo!()
    // let workers = state
    //     .worker_registry_client
    //     .workers()
    //     .await
    //     .map_err(|e| AppError(e))?;
    //
    // Ok((StatusCode::OK, Json(workers)))
}

// Client requests to download an artifact (client only ever downloads proofs)
async fn handle_artifact_download(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    info!("\n=== Received Download Request ===");
    info!("Uri {uri}");

    let uri: Uuid = match Uuid::parse_str(&uri) {
        Ok(u) => u,
        Err(e) => {
            error!("Error parsing uri {uri} as Uuid: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    match state.artifact_store.lock().await.get(&uri) {
        Some(artifact) => {
            let bytes = artifact.bytes.to_vec();
            Ok(bytes)
        }
        None => {
            error!("Artifact {uri} not found");
            Err(StatusCode::NOT_FOUND)
        }
    }
}

// Handler for artifact uploads
async fn handle_artifact_upload(
    State(state): State<Arc<HierophantState>>,
    Path(uri): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    let path = format!("/upload/{}", uri);

    info!("\n=== Received Upload Request ===");
    info!("Path: {}", path);

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

    // TODO: should we remove this from the list of valid urls after its been uploaded?
    //
    // check if this is a valid upload url and get the expected artifact type and uri
    let (artifact_type, uri) = match state.upload_urls.lock().await.get(&path) {
        Some(i) => i.clone(),
        None => {
            error!("Invalid path {path}. Artifact not found");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let artifact = Artifact::new(artifact_type, body);
    state.artifact_store.lock().await.insert(uri, artifact);

    // Return success
    Ok("Upload successful")
}
