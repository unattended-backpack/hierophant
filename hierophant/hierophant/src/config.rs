use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
}

fn default_grpc_port() -> u16 {
    9000
}

fn default_http_port() -> u16 {
    9010
}
