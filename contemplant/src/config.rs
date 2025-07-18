use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    // websocket address in the form of <ws://127.0.0.1:3000/ws>
    pub hierophant_ws_address: String,
    #[serde(default = "default_contemplant_name")]
    pub contemplant_name: String,
    // port for http server
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    // If None, then the contemplant spins up a CUDA prover Docker container.
    #[serde(default = "default_moongate_endpoint")]
    pub moongate_endpoint: Option<String>,
    // How often to tell Hierophant that the contemplant is still alive
    #[serde(default = "default_heartbeat_interval_seconds")]
    pub heartbeat_interval_seconds: u64,
    // max number of finished proofs stored in memory
    #[serde(default = "default_max_proofs_stored")]
    pub max_proofs_stored: usize,
    #[serde(default)]
    pub assessor: AssessorConfig,
    // endpoint to hit to drop this contemplant from it's Magister.
    // Only Some if this contemplant has a Magister
    #[serde(default = "default_magister_drop_endpoint")]
    pub magister_drop_endpoint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssessorConfig {
    // The path to the moongate log file to watch for progress.
    #[serde(default = "default_moongate_log_path")]
    pub moongate_log_path: String,
    // The frequency in milliseconds with which the moongate log watcher should poll the moongate
    // log file.
    #[serde(default = "default_watcher_polling_interval_ms")]
    pub watcher_polling_interval_ms: u64,
}

impl Default for AssessorConfig {
    fn default() -> Self {
        Self {
            moongate_log_path: default_moongate_log_path(),
            watcher_polling_interval_ms: default_watcher_polling_interval_ms(),
        }
    }
}

fn default_http_port() -> u16 {
    9011
}

fn default_magister_drop_endpoint() -> Option<String> {
    None
}

// realistically we shouldn't need more than 1, but some edge cases require us to store more
fn default_max_proofs_stored() -> usize {
    2
}

fn default_moongate_endpoint() -> Option<String> {
    None
}

fn default_heartbeat_interval_seconds() -> u64 {
    30
}

// randomly picks a name from old_testament.txt
fn default_contemplant_name() -> String {
    // name to return if we hit an error during any part of getting random name
    let default_error_name: String = "judas".into();

    let path = Path::new("old_testament.txt");

    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return default_error_name,
    };

    #[allow(clippy::lines_filter_map_ok)]
    let names: Vec<String> = io::BufReader::new(file)
        .lines()
        .filter_map(Result::ok)
        .collect();

    if names.is_empty() {
        return default_error_name;
    }

    let random_index = rand::rng().random_range(0..names.len());

    match names.get(random_index) {
        Some(name) => name.into(),
        None => default_error_name,
    }
}

fn default_moongate_log_path() -> String {
    "./moongate.log".into()
}

fn default_watcher_polling_interval_ms() -> u64 {
    2000
}
