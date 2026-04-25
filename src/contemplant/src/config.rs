use anyhow::{Context, Result};
use network_lib::VmKind;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use std::str::FromStr;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProverBackend {
    Cpu,
    Cuda,
}

impl FromStr for ProverBackend {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "cpu" => Ok(ProverBackend::Cpu),
            "cuda" => Ok(ProverBackend::Cuda),
            _ => anyhow::bail!("Invalid prover backend: '{}'. Must be 'cpu' or 'cuda'", s),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VmChoice {
    Sp1,
    Risc0,
}

impl From<VmChoice> for VmKind {
    fn from(v: VmChoice) -> Self {
        match v {
            VmChoice::Sp1 => VmKind::Sp1,
            VmChoice::Risc0 => VmKind::Risc0,
        }
    }
}

// One entry per VM this contemplant is configured to prove for.  Every entry
// declares its backend; SP1 entries may additionally supply a moongate endpoint
// when running against an external GPU prover.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProverConfig {
    pub vm: VmChoice,
    #[serde(default = "default_backend")]
    pub backend: ProverBackend,
    // Only used for SP1 with a CUDA backend.  If None, sp1-sdk spins up a
    // dockerized moongate-server; otherwise the supplied endpoint is used.
    #[serde(default)]
    pub moongate_endpoint: Option<String>,
    // Only meaningful for `vm = "risc0"`. Opt-in because the RISC Zero
    // Groth16 proving path:
    //   1. requires the ~2.5 GB of vendored prover assets the Dockerfile
    //      installs at /opt/risc0-groth16-prover/ and the
    //      /usr/local/bin/docker shim;
    //   2. is slow on CPU (minutes per proof).
    // An operator running a lean or STARK-only contemplant leaves this false
    // so Groth16 requests fail fast at this worker rather than hanging on
    // missing assets.
    #[serde(default)]
    pub groth16_enabled: bool,
}

fn default_backend() -> ProverBackend {
    ProverBackend::Cpu
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
    // Which VMs this worker serves and how.  At least one entry required.
    #[serde(default)]
    pub provers: Vec<ProverConfig>,
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
            Config {
                hierophant_ws_address: String::new(),
                contemplant_name: default_contemplant_name(),
                http_port: default_http_port(),
                provers: Vec::new(),
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

        // Env-var convenience: CONTEMPLANT_VMS="sp1,risc0" plus optional
        // CONTEMPLANT_SP1_BACKEND / CONTEMPLANT_RISC0_BACKEND / MOONGATE_ENDPOINT.
        // Only applied when the config file didn't supply any `provers` entries.
        if config.provers.is_empty() {
            if let Ok(vms) = env::var("CONTEMPLANT_VMS") {
                for token in vms.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                    match token.to_lowercase().as_str() {
                        "sp1" => {
                            let backend = env::var("CONTEMPLANT_SP1_BACKEND")
                                .ok()
                                .map(|s| s.parse())
                                .transpose()
                                .context("CONTEMPLANT_SP1_BACKEND must be 'cpu' or 'cuda'")?
                                .unwrap_or(ProverBackend::Cpu);
                            let moongate_endpoint = env::var("MOONGATE_ENDPOINT").ok();
                            config.provers.push(ProverConfig {
                                vm: VmChoice::Sp1,
                                backend,
                                moongate_endpoint,
                                groth16_enabled: false,
                            });
                        }
                        "risc0" => {
                            let backend = env::var("CONTEMPLANT_RISC0_BACKEND")
                                .ok()
                                .map(|s| s.parse())
                                .transpose()
                                .context("CONTEMPLANT_RISC0_BACKEND must be 'cpu' or 'cuda'")?
                                .unwrap_or(ProverBackend::Cpu);
                            let groth16_enabled = env::var("CONTEMPLANT_RISC0_GROTH16")
                                .ok()
                                .map(|s| s.parse::<bool>())
                                .transpose()
                                .context("CONTEMPLANT_RISC0_GROTH16 must be 'true' or 'false'")?
                                .unwrap_or(false);
                            config.provers.push(ProverConfig {
                                vm: VmChoice::Risc0,
                                backend,
                                moongate_endpoint: None,
                                groth16_enabled,
                            });
                        }
                        other => anyhow::bail!(
                            "Unknown VM '{other}' in CONTEMPLANT_VMS (expected sp1, risc0)"
                        ),
                    }
                }
            }
        }

        // Validate required fields
        if config.hierophant_ws_address.is_empty() {
            anyhow::bail!(
                "hierophant_ws_address is required. Provide it via config file or HIEROPHANT_WS_ADDRESS environment variable."
            );
        }
        if config.provers.is_empty() {
            anyhow::bail!(
                "At least one [[provers]] entry is required. Declare which VM(s) this contemplant serves."
            );
        }

        // RISC Zero CUDA support is a cargo-feature-gated opt-in: the binary
        // must be built with `--features enable-risc0-cuda` so that the
        // `risc0-zkvm/cuda` bindings are linked in. Reject at startup when a
        // CUDA-less build gets a CUDA config, so the operator sees a clean
        // error instead of a silent CPU fallback.
        for prover in &config.provers {
            #[cfg(not(feature = "enable-risc0-cuda"))]
            if matches!(prover.vm, VmChoice::Risc0) && matches!(prover.backend, ProverBackend::Cuda)
            {
                anyhow::bail!(
                    "RISC Zero CUDA backend requires building the contemplant with `--features enable-risc0-cuda`. \
                     Rebuild with that feature (and run the container with GPU access, e.g. `--gpus all`) \
                     or set backend = \"cpu\" on the risc0 [[provers]] entry."
                );
            }
            if prover.groth16_enabled && !matches!(prover.vm, VmChoice::Risc0) {
                anyhow::bail!(
                    "groth16_enabled is only meaningful for vm = \"risc0\"; saw it on {:?}.",
                    prover.vm
                );
            }
        }

        Ok(config)
    }
}
