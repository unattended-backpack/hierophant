mod artifact_store;
mod config;
mod proof_router;

mod create_artifact_service;
mod hierophant_state;
mod http_handler;
mod prover_network_service;
mod ws_handler;
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
use axum::extract::DefaultBodyLimit;
use create_artifact_service::ArtifactStoreService;
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
    let artifact_service = ArtifactStoreService::new(hierophant_state.clone());

    info!("gRPC server starting on {grpc_addr}");
    let grpc_server = Server::builder()
        .add_service(ProverNetworkServer::new(prover_service))
        .add_service(ArtifactStoreServer::new(artifact_service))
        .serve(grpc_addr);

    // Create the axum router with all routes
    let app =
        http_handler::create_router(hierophant_state.clone()).layer(DefaultBodyLimit::disable());

    // Run the HTTP server in a separate task
    let http_server = tokio::spawn(async move {
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

    info!("Starting Hierophant services:");
    info!("  - gRPC server on {grpc_addr}");
    info!("  - HTTP (/ws) server on {http_addr}");
    info!("Servers started. Press Ctrl+C to stop.");

    // Wait for both servers to complete (or error)
    tokio::select! {
        _ = grpc_server => info!("gRPC server terminated"),
        _ = http_server => info!("HTTP server terminated"),
    }

    Ok(())
}
