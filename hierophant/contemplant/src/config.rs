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
    // If None, then the contemplant spins up a CUDA prover Docker container.
    #[serde(default = "default_moongate_endpoint")]
    pub moongate_endpoint: Option<String>,
    // How often to tell Hierophant that the contemplant is still alive
    #[serde(default = "default_heartbeat_interval_seconds")]
    pub heartbeat_interval_seconds: u64,
    // max number of finished proofs stored in memory
    #[serde(default = "default_max_proofs_stored")]
    pub max_proofs_stored: usize,
    #[serde(default = "default_assessor_config")]
    pub assessor: AssessorConfig,
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

    let file = match File::open(&path) {
        Ok(file) => file,
        Err(_) => return default_error_name,
    };

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

fn default_assessor_config() -> AssessorConfig {
    AssessorConfig {
        moongate_log_path: default_moongate_log_path(),
        watcher_reporting_interval_ms: default_watcher_reporting_inteval_ms(),
        watcher_polling_interval_ms: default_watcher_polling_interval_ms(),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssessorConfig {
    // The path to the moongate log file to watch for progress.
    #[serde(default = "default_moongate_log_path")]
    pub moongate_log_path: String,
    // The frequency in milliseconds with which the moongate log watcher should report progress.
    #[serde(default = "default_watcher_reporting_inteval_ms")]
    pub watcher_reporting_interval_ms: u64,
    // The frequency in milliseconds with which the moongate log watcher should poll the moongate
    // log file.
    #[serde(default = "default_watcher_polling_interval_ms")]
    pub watcher_polling_interval_ms: u64,
}

fn default_moongate_log_path() -> String {
    "./moongate.log".into()
}

fn default_watcher_reporting_inteval_ms() -> u64 {
    1000
}

fn default_watcher_polling_interval_ms() -> u64 {
    100
}
