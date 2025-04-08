mod config;

mod hierophant_state;
mod http_handler;
mod services;
pub mod network {
    tonic::include_proto!("network");
}
pub mod artifact {
    tonic::include_proto!("artifact");
}

use crate::config::Config;
use crate::hierophant_state::HierophantState;
use anyhow::Context;
use artifact::artifact_store_server::ArtifactStoreServer;
use network::prover_network_server::ProverNetworkServer;
use services::{ArtifactStoreService, ProverNetworkService};
use std::sync::Arc;
use tonic::transport::Server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = tokio::fs::read_to_string("hierophant.toml")
        .await
        .context("read hierophant.toml file")?;

    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Create a structure for sharing all application state.
    let hierophant_state = Arc::new(HierophantState::new(config));

    // Define the server addresses
    // TODO: address/ports from .env
    let grpc_addr = ([0, 0, 0, 0], 9009).into();
    // let http_addr = ([0, 0, 0, 0], 9010).into();

    // Create the gRPC services with access to shared state.
    let prover_service = ProverNetworkService::new(hierophant_state.clone());
    let artifact_service = ArtifactStoreService::new(hierophant_state.clone());

    // Run the gRPC server
    let grpc_server = Server::builder()
        .add_service(ProverNetworkServer::new(prover_service))
        .add_service(ArtifactStoreServer::new(artifact_service))
        .serve(grpc_addr);

    // Create the axum router with all routes
    let app = http_handler::create_router(hierophant_state.clone());

    // Run the HTTP server in a separate task
    let http_server = tokio::spawn(async move {
        println!("HTTP server starting on 0.0.0.0:9010");
        axum::serve(
            tokio::net::TcpListener::bind("0.0.0.0:9010").await.unwrap(),
            app,
        )
        .await
        .unwrap();
    });

    println!("Starting Hierophant services:");
    println!("  - gRPC server on http://0.0.0.0:9009");
    println!("  - HTTP server on http://0.0.0.0:9010");
    println!("Implemented methods:");
    println!("  - network.ProverNetwork/GetProgram");
    println!("  - network.ProverNetwork/GetNonce");
    println!("  - network.ProverNetwork/CreateProgram");
    println!("  - network.ProverNetwork/RequestProof");
    println!("  - network.ProverNetwork/GetProofRequestStatus");
    println!("  - artifact.ArtifactStore/CreateArtifact");
    println!("  - HTTP POST/PUT to /upload/:id (for artifact uploads)");
    println!("  - HTTP PUT to /worker (for contemplant registration)");
    println!("Servers started. Press Ctrl+C to stop.");

    // Wait for both servers to complete (or error)
    tokio::select! {
        _ = grpc_server => println!("gRPC server terminated"),
        _ = http_server => println!("HTTP server terminated"),
    }

    Ok(())
}
