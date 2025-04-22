use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub hierophant_address: String,
    pub contemplant_name: String,
    #[serde(default = "default_port")]
    pub port: usize,
}

fn default_port() -> usize {
    3000
}
