mod artifact_store;
mod config;
mod proof_router;

mod create_artifact_service;
mod hierophant_state;
mod http_handler;
mod prover_network_service;
pub mod network {
    tonic::include_proto!("network");
}
pub mod artifact {
    tonic::include_proto!("artifact");
}
use crate::config::Config;
use crate::hierophant_state::HierophantState;
use anyhow::Context;
use artifact::create_artifact_server::CreateArtifactServer;
use create_artifact_service::CreateArtifactService;
use log::info;
use network::prover_network_server::ProverNetworkServer;
use prover_network_service::ProverNetworkService;
use std::{net::SocketAddr, sync::Arc};
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config = tokio::fs::read_to_string("hierophant.toml")
        .await
        .context("read hierophant.toml file")?;

    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Create a structure for sharing all application state.
    let hierophant_state = Arc::new(HierophantState::new(config.clone()));

    // Define the server addresses
    let grpc_addr: SocketAddr = ([0, 0, 0, 0], config.grpc_port).into();
    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();

    // Create the gRPC services with access to shared state.
    let prover_service = ProverNetworkService::new(hierophant_state.clone());
    let artifact_service = CreateArtifactService::new(hierophant_state.clone());

    // Run the gRPC server
    info!("gRPC server starting on {http_addr}");
    let grpc_server = Server::builder()
        .add_service(ProverNetworkServer::new(prover_service))
        .add_service(CreateArtifactServer::new(artifact_service))
        .serve(grpc_addr);

    // Create the axum router with all routes
    let app = http_handler::create_router(hierophant_state.clone());

    // Run the HTTP server in a separate task
    let http_server = tokio::spawn(async move {
        info!("HTTP server starting on {http_addr}");
        axum::serve(
            tokio::net::TcpListener::bind(http_addr)
                .await
                .context("bind http server to {http_addr}")
                .unwrap(),
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .context("Axum serve on {http_addr}")
        .unwrap();
    });

    println!("Starting Hierophant services:");
    println!("  - gRPC server on {grpc_addr}");
    println!("  - HTTP server on {http_addr}");
    println!("Implemented methods:");
    println!("  - network.ProverNetwork/GetProgram");
    println!("  - network.ProverNetwork/GetNonce");
    println!("  - network.ProverNetwork/CreateProgram");
    println!("  - network.ProverNetwork/RequestProof");
    println!("  - network.ProverNetwork/GetProofRequestStatus");
    println!("  - artifact.CreateArtifact/CreateArtifact");
    println!("  - HTTP POST/PUT to /:id (for artifact uploads)");
    println!("  - HTTP GET to /:id (for artifact downloads)");
    println!("  - HTTP PUT to /register_worker (for contemplant registration)");
    println!("Servers started. Press Ctrl+C to stop.");

    // Wait for both servers to complete (or error)
    tokio::select! {
        _ = grpc_server => println!("gRPC server terminated"),
        _ = http_server => println!("HTTP server terminated"),
    }

    Ok(())
}
