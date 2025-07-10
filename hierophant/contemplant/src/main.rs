mod config;

mod connection;
mod message_handler;
mod proof_executor;
mod proof_store;
mod worker_state;

use crate::config::Config;
use crate::worker_state::WorkerState;
use anyhow::{Context, Result};
use log::info;
use sp1_sdk::utils;

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

    connection::connect_to_hierophant(config, worker_state).await?;

    info!("Shutting down");

    Ok(())
}
