mod config;

mod api;
mod message_handler;
mod proof_executor;
mod proof_store;
mod worker_state;

use crate::config::Config;
use crate::worker_state::WorkerState;
use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, info};
use sp1_sdk::utils;
use std::{net::SocketAddr, sync::Arc};

// used for dynamic environments that use multiple configurations, like running an integration test
// on a machine that has another config
#[derive(Parser)]
struct Args {
    /// Path to config file
    #[arg(short, long, default_value = "contemplant.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_file = Args::parse().config;
    debug!("Using config {config_file}");

    let config = Config::load(&config_file).context("load configuration")?;

    // Set up the SP1 SDK logger.
    utils::setup_logger();

    info!("Starting contemplant {}", config.contemplant_name);

    let worker_state = WorkerState::new(config.clone());

    // Create a broadcast channel for shutdown signal
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();

    // Spawn a task to listen for SIGINT or SIGTERM and broadcast shutdown.
    // Handling SIGTERM gives us a chance to send a clean WebSocket Close
    // frame to hierophant on `docker stop` / `docker-compose down` before
    // docker's 10s grace period elapses and SIGKILL's us; otherwise
    // hierophant logs the TCP reset as an ERROR.
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let sigterm = signal(SignalKind::terminate());
        let mut sigterm = match sigterm {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Failed to install SIGTERM handler: {e}. Only SIGINT will trigger graceful shutdown.");
                let _ = tokio::signal::ctrl_c().await;
                info!("Received SIGINT, stopping services...");
                let _ = shutdown_tx_clone.send(());
                return;
            }
        };
        tokio::select! {
            r = tokio::signal::ctrl_c() => {
                if let Err(e) = r {
                    log::warn!("SIGINT handler error: {e}");
                }
                info!("Received SIGINT, stopping services...");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, stopping services...");
            }
        }
        let _ = shutdown_tx_clone.send(());
    });

    let worker_state_clone = worker_state.clone();
    let config_clone = config.clone();
    let ws_shutdown_rx = shutdown_tx.subscribe();
    let hierophant_ws = tokio::spawn(async move {
        if let Err(e) = api::connect_to_hierophant(config_clone, worker_state_clone, ws_shutdown_rx)
            .await
            .context("hierophant ws connection")
        {
            error!("Error in Hierophant connection channel: {e}");
        }
    });

    // Create the axum http router with all routes
    let app = api::create_router(Arc::new(worker_state.clone()));

    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();

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

    info!("Http server listening on {http_addr}");

    // Wait for both tasks
    tokio::select! {
        _ = hierophant_ws => info!("WebSocket connection with Hierophant has been terminated"),
        _ = http_server => info!("HTTP server shutdown complete"),
    }

    Ok(())
}
