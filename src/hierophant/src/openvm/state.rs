use alloy_primitives::B256;
use network_lib::OpenVmProofMode;
use std::collections::HashMap;
use tokio::sync::Mutex;

// In-memory bookkeeping for OpenVM-shaped requests, mirroring BonsaiState.
// Programs and inputs are stored uploaded-once-and-cached. Proof bytes get
// stored when a job is observed successful on status poll. Jobs map 1:1 to a
// hierophant proof request_id so we can look up worker status through the
// existing ProofRouter.
pub struct OpenVmState {
    pub inner: Mutex<OpenVmInner>,
}

pub struct OpenVmInner {
    // program_id (sha256 hex digest of the ELF) -> raw guest ELF bytes
    pub programs: HashMap<String, Vec<u8>>,
    // input uuid -> input streams (one entry per StdIn hint stream)
    pub inputs: HashMap<String, Vec<Vec<u8>>>,
    // proof uuid -> job metadata
    pub jobs: HashMap<String, OpenVmJob>,
    // proof uuid -> serialized proof bytes (populated when the job's
    // underlying proof completes and passes hierophant-side verification)
    pub proofs: HashMap<String, Vec<u8>>,
}

pub struct OpenVmJob {
    // The hierophant request_id used to poll the worker registry for status
    // and retrieve the proof when complete.
    pub request_id: B256,
    // sha256 hex digest of the guest ELF this job proves; used to look the
    // ELF back up at verification time.
    pub program_id: String,
    // Optional `openvm.toml` app-config contents forwarded by the client.
    // Retained so verification derives keys from the exact same config the
    // contemplant proved against.
    pub app_config_toml: Option<String>,
    pub mode: OpenVmProofMode,
    // terminal error surfaced to GET /proofs/status/{uuid} on failure
    pub terminal_error: Option<String>,
}

impl Default for OpenVmState {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenVmState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(OpenVmInner {
                programs: HashMap::new(),
                inputs: HashMap::new(),
                jobs: HashMap::new(),
                proofs: HashMap::new(),
            }),
        }
    }
}
