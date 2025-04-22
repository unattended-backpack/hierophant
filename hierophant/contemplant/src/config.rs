use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub hierophant_address: String,
    pub contemplant_name: String,
    #[serde(default = "default_port")]
    pub port: usize,
    // If None, then the contemplant spins up a cuda prover docker container
    #[serde(default = "default_moongate_endpoint")]
    pub moongate_endpoint: Option<String>,
}

fn default_moongate_endpoint() -> Option<String> {
    None
}

fn default_port() -> usize {
    3000
}
