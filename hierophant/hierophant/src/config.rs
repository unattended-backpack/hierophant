use std::time::Duration;

use alloy_primitives::Address;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
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
    // How long to wait for a response from a worker before evicting them.
    // For example, Hierophant waiting for a response from a worker on a proof_status_request
    #[serde(
        default = "default_worker_response_timeout_secs",
        deserialize_with = "deserialize_duration_from_secs"
    )]
    pub worker_response_timeout_secs: Duration,
    // publicly reachable address of this Hierophant for artifact uploads
    // TODO: remove, should discover this
    pub this_hierophant_ip: String,
    // key pair used for signing messages to the client and retreiving nonces
    pub pub_key: Address,
    // Make mock proofs instead of real proofs.  Witnessgen still happens.
    #[serde(default = "default_mock_mode")]
    pub mock_mode: bool,
    // The amount of proofs to cache on-disk for persistence.
    // Keeps most recently completed proofs.
    #[serde(default = "default_proof_cache_size")]
    pub proof_cache_size: usize,
    // Where to store cached proofs on-disk
    #[serde(default = "default_proof_cache_directory")]
    pub proof_cache_directory: String,
    // Where artifacts are stored on-disk
    #[serde(default = "default_artifact_store_directory")]
    pub artifact_store_directory: String,
}

fn default_worker_response_timeout_secs() -> Duration {
    // If the worker doesn't response within 20 seconds they're likely dead
    Duration::from_secs(20)
}

fn default_max_worker_heartbeat_interval_secs() -> Duration {
    // Give the workers a large berth by default, 3 mins
    Duration::from_secs(3 * 60)
}

fn default_proof_cache_directory() -> String {
    "proofs".into()
}

fn default_artifact_store_directory() -> String {
    "artifacts".into()
}

fn default_proof_cache_size() -> usize {
    10
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
