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
    // Maximum time a contemplant can be working on a proof before they're declared
    // probably dead and kicked
    #[serde(default = "proof_timeout_mins")]
    pub proof_timeout_mins: u64,
    // publicly reachable address of this Hierophant for artifact uploads
    pub this_hierophant_ip: String,
    // key pair used for signing messages to the client and retreiving nonces
    pub pub_key: Address,
    // Make mock proofs instead of real proofs.  Witnessgen still happens.
    #[serde(default = "default_mock_mode")]
    pub mock_mode: bool,
    // Where artifacts are stored on-disk
    #[serde(default = "default_artifact_store_directory")]
    pub artifact_store_directory: String,
    // Artifacts can be quite large so we need to limit how many we store on-disk inside the
    // artifact_store_directory
    #[serde(default = "default_max_artifacts_stored")]
    pub max_artifacts_stored: usize,
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
fn default_max_artifacts_stored() -> usize {
    50
}

fn proof_timeout_mins() -> u64 {
    // 2 hours
    60 * 2
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
