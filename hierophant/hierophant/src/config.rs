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
