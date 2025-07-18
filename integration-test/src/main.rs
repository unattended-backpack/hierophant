use anyhow::Result;
use log::{error, info};
use std::process::Command;
use tokio::time::{Duration, sleep};

#[tokio::main]
async fn main() {
    env_logger::init();
    info!("Running integration test");

    // TODO:
    /*
        // Start Hierophant
        let mut hierophant = Command::new("cargo")
            .args(&[
                "run",
                "--release",
                "--bin",
                "hierophant",
                "--",
                "--config integration-test/test-hierophant.toml",
            ])
            .spawn()
            .expect("Failed to start Hierophant");

        // Wait for Hierophant to be ready
        sleep(Duration::from_secs(2)).await;

        // Start Contemplant
        let mut contemplant = Command::new("cargo")
            .args(&[
                "run",
                "--release",
                "--bin",
                "contemplant",
                "--",
                "--config integration-test/test-contemplant.toml",
            ])
            .spawn()
            .expect("Failed to start Contemplant");

        if let Err(e) = run_test().await {
            error!("Test failed: {e}");
        }

        // Cleanup
        hierophant.kill().ok();
        contemplant.kill().ok();
    */

    // Remove temp files
    //
}

async fn run_test() -> Result<()> {
    Ok(())
}
