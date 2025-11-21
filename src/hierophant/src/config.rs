use std::env;
use std::path::Path;
use std::time::Duration;

use alloy_primitives::Address;
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    // Also the ws port for the contemplant config
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    // How long to wait for a response from a worker before evicting them.
    // For example, Hierophant waiting for a response from a worker on a proof_status_request
    #[serde(
        default = "default_worker_response_timeout_secs",
        deserialize_with = "deserialize_duration_from_secs"
    )]
    pub worker_response_timeout_secs: Duration,
    // publicly reachable address of this Hierophant for artifact uploads
    pub this_hierophant_ip: String,
    // TODO: Require a signing key and remove default.  This is a future feature to be implemented.
    // key pair used for signing messages to the client and retreiving nonces
    #[serde(default = "default_pub_key")]
    pub pub_key: Address,
    // Make mock proofs instead of real proofs.  Witnessgen still happens.
    #[serde(default = "default_mock_mode")]
    pub mock_mode: bool,
    // Where artifacts are stored on-disk
    #[serde(default = "default_artifact_store_directory")]
    pub artifact_store_directory: String,
    // Artifacts can be quite large so we need to limit how many we store on-disk inside the
    // artifact_store_directory
    #[serde(default = "default_max_stdin_artifacts_stored")]
    pub max_stdin_artifacts_stored: usize,
    #[serde(default = "default_max_proof_artifacts_stored")]
    pub max_proof_artifacts_stored: usize,
    // Conditions to drop workers
    #[serde(default)]
    pub worker_registry: WorkerRegistryConfig,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkerRegistryConfig {
    // number of strikes before a worker is evicted.  Typically a worker gets a strike
    // when it fails to respond to a request
    #[serde(default = "default_max_worker_strikes")]
    pub max_worker_strikes: usize,
    // How long between worker heartbeats to wait before evicting workers.
    // For reference, the default worker heartbeat is 30 seconds.
    #[serde(
        default = "default_max_worker_heartbeat_interval_secs",
        deserialize_with = "deserialize_duration_from_secs"
    )]
    pub max_worker_heartbeat_interval_secs: Duration,
    // Maximum time a contemplant can be working on a proof before they're declared
    // probably dead and kicked
    #[serde(default = "default_proof_timeout_mins")]
    pub proof_timeout_mins: u64,
    // The contemplant must make at least 1% progress every contemplant_required_progress_interval_mins
    // or it will be dropped
    #[serde(default = "default_worker_required_progress_interval_mins")]
    pub worker_required_progress_interval_mins: u64,
    // The amount of time the execution report can be running for the contemplant.
    // This is measured by the contemplant returning None progress.  When it returns
    // Some() then the execution report is done and the proof has started executing.
    #[serde(default = "default_worker_max_execution_report_mins")]
    pub worker_max_execution_report_mins: u64,
}

impl Default for WorkerRegistryConfig {
    fn default() -> Self {
        Self {
            max_worker_strikes: default_max_worker_strikes(),
            max_worker_heartbeat_interval_secs: default_max_worker_heartbeat_interval_secs(),
            proof_timeout_mins: default_proof_timeout_mins(),
            worker_required_progress_interval_mins: default_worker_required_progress_interval_mins(
            ),
            worker_max_execution_report_mins: default_worker_max_execution_report_mins(),
        }
    }
}

fn default_pub_key() -> Address {
    Address::default()
}

fn default_worker_max_execution_report_mins() -> u64 {
    45
}

fn default_worker_required_progress_interval_mins() -> u64 {
    0
}

fn default_worker_response_timeout_secs() -> Duration {
    Duration::from_secs(30)
}

fn default_max_worker_heartbeat_interval_secs() -> Duration {
    // Give the workers a large berth by default, 3 mins
    Duration::from_secs(3 * 60)
}

fn default_artifact_store_directory() -> String {
    "artifacts".into()
}

// We need to give a default to have SOME upper limit to how big the `artifact_store_directory`
// can grow
fn default_max_stdin_artifacts_stored() -> usize {
    50
}

fn default_max_proof_artifacts_stored() -> usize {
    10
}

fn default_proof_timeout_mins() -> u64 {
    // 5 hours.  They're more likely to get cut off because they're not making progress
    60 * 5
}

fn default_mock_mode() -> bool {
    false
}

fn default_max_worker_strikes() -> usize {
    3
}

fn default_grpc_port() -> u16 {
    9000
}

fn default_http_port() -> u16 {
    9010
}

// Custom deserializer function for Duration from seconds
fn deserialize_duration_from_secs<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    // This will attempt to deserialize the input as a u64
    let seconds = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(seconds))
}

impl Config {
    /// Load configuration from .toml file and/or environment variables.
    /// Priority: environment variables > .toml file > defaults
    /// The .toml file is optional if all required fields are provided via environment variables.
    pub fn load(config_path: &str) -> Result<Self> {
        // Try to load from file if it exists
        let mut config = if Path::new(config_path).exists() {
            let contents = std::fs::read_to_string(config_path)
                .context(format!("Failed to read config file: {}", config_path))?;
            toml::from_str::<Config>(&contents)
                .context(format!("Failed to parse config file: {}", config_path))?
        } else {
            // No file exists, create config with defaults (this_hierophant_ip will be empty)
            Config {
                grpc_port: default_grpc_port(),
                http_port: default_http_port(),
                worker_response_timeout_secs: default_worker_response_timeout_secs(),
                this_hierophant_ip: String::new(),
                pub_key: default_pub_key(),
                mock_mode: default_mock_mode(),
                artifact_store_directory: default_artifact_store_directory(),
                max_stdin_artifacts_stored: default_max_stdin_artifacts_stored(),
                max_proof_artifacts_stored: default_max_proof_artifacts_stored(),
                worker_registry: WorkerRegistryConfig::default(),
            }
        };

        // Override with environment variables if present
        if let Ok(val) = env::var("GRPC_PORT") {
            config.grpc_port = val.parse().context("GRPC_PORT must be a valid u16")?;
        }
        if let Ok(val) = env::var("HTTP_PORT") {
            config.http_port = val.parse().context("HTTP_PORT must be a valid u16")?;
        }
        if let Ok(val) = env::var("WORKER_RESPONSE_TIMEOUT_SECS") {
            let secs: u64 = val
                .parse()
                .context("WORKER_RESPONSE_TIMEOUT_SECS must be a valid u64")?;
            config.worker_response_timeout_secs = Duration::from_secs(secs);
        }
        if let Ok(val) = env::var("THIS_HIEROPHANT_IP") {
            config.this_hierophant_ip = val;
        }
        if let Ok(val) = env::var("PUB_KEY") {
            config.pub_key = val
                .parse()
                .context("PUB_KEY must be a valid Ethereum address")?;
        }
        if let Ok(val) = env::var("MOCK_MODE") {
            config.mock_mode = val
                .parse()
                .context("MOCK_MODE must be 'true' or 'false'")?;
        }
        if let Ok(val) = env::var("ARTIFACT_STORE_DIRECTORY") {
            config.artifact_store_directory = val;
        }
        if let Ok(val) = env::var("MAX_STDIN_ARTIFACTS_STORED") {
            config.max_stdin_artifacts_stored = val
                .parse()
                .context("MAX_STDIN_ARTIFACTS_STORED must be a valid usize")?;
        }
        if let Ok(val) = env::var("MAX_PROOF_ARTIFACTS_STORED") {
            config.max_proof_artifacts_stored = val
                .parse()
                .context("MAX_PROOF_ARTIFACTS_STORED must be a valid usize")?;
        }

        // Worker registry config overrides
        if let Ok(val) = env::var("MAX_WORKER_STRIKES") {
            config.worker_registry.max_worker_strikes = val
                .parse()
                .context("MAX_WORKER_STRIKES must be a valid usize")?;
        }
        if let Ok(val) = env::var("MAX_WORKER_HEARTBEAT_INTERVAL_SECS") {
            let secs: u64 = val
                .parse()
                .context("MAX_WORKER_HEARTBEAT_INTERVAL_SECS must be a valid u64")?;
            config.worker_registry.max_worker_heartbeat_interval_secs = Duration::from_secs(secs);
        }
        if let Ok(val) = env::var("PROOF_TIMEOUT_MINS") {
            config.worker_registry.proof_timeout_mins = val
                .parse()
                .context("PROOF_TIMEOUT_MINS must be a valid u64")?;
        }
        if let Ok(val) = env::var("WORKER_REQUIRED_PROGRESS_INTERVAL_MINS") {
            config.worker_registry.worker_required_progress_interval_mins = val
                .parse()
                .context("WORKER_REQUIRED_PROGRESS_INTERVAL_MINS must be a valid u64")?;
        }
        if let Ok(val) = env::var("WORKER_MAX_EXECUTION_REPORT_MINS") {
            config.worker_registry.worker_max_execution_report_mins = val
                .parse()
                .context("WORKER_MAX_EXECUTION_REPORT_MINS must be a valid u64")?;
        }

        // Validate required fields
        if config.this_hierophant_ip.is_empty() {
            anyhow::bail!(
                "this_hierophant_ip is required. Provide it via config file or THIS_HIEROPHANT_IP environment variable."
            );
        }

        Ok(config)
    }
}
