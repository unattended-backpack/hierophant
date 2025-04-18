use alloy_primitives::Address;
use serde::{Deserialize, Serialize};

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
