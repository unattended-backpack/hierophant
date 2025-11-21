mod api;
mod artifact_store;
mod config;
mod proof;
mod worker_registry;

mod hierophant_state;
pub mod network {
    tonic::include_proto!("network");
}
pub mod artifact {
    tonic::include_proto!("artifact");
}
use crate::config::Config;
use crate::hierophant_state::HierophantState;
use anyhow::Context;
use api::{
    grpc::{
        create_artifact_service::ArtifactStoreService, prover_network_service::ProverNetworkService,
    },
    http::create_router,
};

use artifact::artifact_store_server::ArtifactStoreServer;
use axum::extract::DefaultBodyLimit;
use clap::Parser;
use log::{debug, info};
use network::prover_network_server::ProverNetworkServer;
use std::{net::SocketAddr, sync::Arc};
use tonic::transport::Server;

// used for dynamic environments that use multiple configurations, like running an integration test
// on a machine that has another config
#[derive(Parser)]
struct Args {
    /// Path to config file
    #[arg(short, long, default_value = "hierophant.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let config_file = Args::parse().config;
    debug!("Using config {config_file}");

    let config = Config::load(&config_file).context("load configuration")?;

    // Create a structure for sharing all application state.
    let hierophant_state = Arc::new(HierophantState::new(config.clone()));

    // Define the server addresses
    let grpc_addr: SocketAddr = ([0, 0, 0, 0], config.grpc_port).into();
    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();

    // Create the gRPC services with access to shared state.
    let prover_service = ProverNetworkService::new(hierophant_state.clone());
    let artifact_service = ArtifactStoreService::new(hierophant_state.clone());

    // Create a broadcast channel for shutdown signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();

    // Spawn a task to listen for ctrl+c and broadcast shutdown
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install CTRL+C signal handler");
        info!("Received shutdown signal, stopping servers...");
        let _ = shutdown_tx_clone.send(());
    });

    // Create shutdown signal handler for gRPC server
    let mut grpc_shutdown_rx = shutdown_tx.subscribe();
    let grpc_shutdown_signal = async move {
        let _ = grpc_shutdown_rx.recv().await;
    };

    info!("gRPC server starting on {grpc_addr}");
    let grpc_server = Server::builder()
        .add_service(ProverNetworkServer::new(prover_service))
        .add_service(ArtifactStoreServer::new(artifact_service))
        .serve_with_shutdown(grpc_addr, grpc_shutdown_signal);

    // Create the axum http router with all routes
    let app = create_router(hierophant_state.clone()).layer(DefaultBodyLimit::disable());

    // Create shutdown signal handler for HTTP server
    let mut http_shutdown_rx = shutdown_tx.subscribe();
    let http_shutdown_signal = async move {
        let _ = http_shutdown_rx.recv().await;
    };

    // Run the HTTP server with graceful shutdown
    let http_server = tokio::spawn(async move {
        axum::serve(
            tokio::net::TcpListener::bind(http_addr)
                .await
                .context("bind http server to {http_addr}")
                .unwrap(),
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(http_shutdown_signal)
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
        result = grpc_server => {
            match result {
                Ok(_) => info!("gRPC server shutdown complete"),
                Err(e) => info!("gRPC server error: {}", e),
            }
        },
        _ = http_server => info!("HTTP server shutdown complete"),
    }

    Ok(())
}
