mod config;

mod api;
mod message_handler;
mod proof_executor;
mod proof_store;
mod worker_state;

use crate::config::Config;
use crate::worker_state::WorkerState;
use anyhow::{Context, Result};
use log::{error, info};
use sp1_sdk::utils;
use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> Result<()> {
    let config = tokio::fs::read_to_string("contemplant.toml")
        .await
        .context("read contemplant.toml file")?;

    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Set up the SP1 SDK logger.
    utils::setup_logger();

    info!("Starting contemplant {}", config.contemplant_name);

    let worker_state = WorkerState::new(config.clone());

    let worker_state_clone = worker_state.clone();
    let config_clone = config.clone();
    let hierophant_ws = tokio::spawn(async move {
        if let Err(e) = api::connect_to_hierophant(config_clone, worker_state_clone)
            .await
            .context("hierophant ws connection")
        {
            error!("Error in Hierophant connection channel: {e}");
        }
    });

    // Create the axum http router with all routes
    let app = api::create_router(Arc::new(worker_state.clone()));

    let http_addr: SocketAddr = ([0, 0, 0, 0], config.http_port).into();
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

    info!("Http server listening on {http_addr}");

    // Wait for both tasks
    tokio::select! {
        _ = hierophant_ws => info!("WebSocket connection with Hierophant has been terminated"),
        _ = http_server => info!("HTTP server terminated"),
    }

    Ok(())
}
