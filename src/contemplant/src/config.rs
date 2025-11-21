use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProverType {
    Cpu,
    Cuda,
}

impl FromStr for ProverType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "cpu" => Ok(ProverType::Cpu),
            "cuda" => Ok(ProverType::Cuda),
            _ => anyhow::bail!("Invalid prover type: '{}'. Must be 'cpu' or 'cuda'", s),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    // websocket address in the form of <ws://127.0.0.1:3000/ws>
    pub hierophant_ws_address: String,
    #[serde(default = "default_contemplant_name")]
    pub contemplant_name: String,
    // port for http server
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    // Prover type: cpu or cuda
    #[serde(default = "default_prover_type")]
    pub prover_type: ProverType,
    // If None, then the contemplant spins up a CUDA prover Docker container.
    // Only used when prover_type is Cuda.
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

fn default_prover_type() -> ProverType {
    ProverType::Cpu
}

fn default_moongate_endpoint() -> Option<String> {
    None
}

fn default_heartbeat_interval_seconds() -> u64 {
    30
}

// randomly picks a name from names.txt
fn default_contemplant_name() -> String {
    // name to return if we hit an error during any part of getting random name
    let default_error_name: String = "judas".into();

    let path = Path::new("names.txt");

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
            // No file exists, create config with defaults (hierophant_ws_address will be empty)
            Config {
                hierophant_ws_address: String::new(),
                contemplant_name: default_contemplant_name(),
                http_port: default_http_port(),
                prover_type: default_prover_type(),
                moongate_endpoint: default_moongate_endpoint(),
                heartbeat_interval_seconds: default_heartbeat_interval_seconds(),
                max_proofs_stored: default_max_proofs_stored(),
                assessor: AssessorConfig::default(),
                magister_drop_endpoint: default_magister_drop_endpoint(),
            }
        };

        // Override with environment variables if present
        if let Ok(val) = env::var("HIEROPHANT_WS_ADDRESS") {
            config.hierophant_ws_address = val;
        }
        if let Ok(val) = env::var("CONTEMPLANT_NAME") {
            config.contemplant_name = val;
        }
        if let Ok(val) = env::var("HTTP_PORT") {
            config.http_port = val.parse().context("HTTP_PORT must be a valid u16")?;
        }
        if let Ok(val) = env::var("PROVER_TYPE") {
            config.prover_type = val.to_lowercase().parse().context(
                "PROVER_TYPE must be either 'cpu' or 'cuda'"
            )?;
        }
        if let Ok(val) = env::var("MOONGATE_ENDPOINT") {
            config.moongate_endpoint = Some(val);
        }
        if let Ok(val) = env::var("HEARTBEAT_INTERVAL_SECONDS") {
            config.heartbeat_interval_seconds = val
                .parse()
                .context("HEARTBEAT_INTERVAL_SECONDS must be a valid u64")?;
        }
        if let Ok(val) = env::var("MAX_PROOFS_STORED") {
            config.max_proofs_stored = val
                .parse()
                .context("MAX_PROOFS_STORED must be a valid usize")?;
        }
        if let Ok(val) = env::var("MAGISTER_DROP_ENDPOINT") {
            config.magister_drop_endpoint = Some(val);
        }
        if let Ok(val) = env::var("MOONGATE_LOG_PATH") {
            config.assessor.moongate_log_path = val;
        }
        if let Ok(val) = env::var("WATCHER_POLLING_INTERVAL_MS") {
            config.assessor.watcher_polling_interval_ms = val
                .parse()
                .context("WATCHER_POLLING_INTERVAL_MS must be a valid u64")?;
        }

        // Validate required fields
        if config.hierophant_ws_address.is_empty() {
            anyhow::bail!(
                "hierophant_ws_address is required. Provide it via config file or HIEROPHANT_WS_ADDRESS environment variable."
            );
        }

        Ok(config)
    }
}
