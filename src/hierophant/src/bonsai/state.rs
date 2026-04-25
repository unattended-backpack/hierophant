use alloy_primitives::B256;
use network_lib::Risc0ProofMode;
use std::collections::HashMap;
use tokio::sync::Mutex;

// In-memory bookkeeping for Bonsai-shaped requests. Images and inputs are
// stored uploaded-once-and-cached, identical to how Bonsai treats them on the
// wire. Receipts get stored when a session is observed successful on status
// poll. Sessions map 1:1 to a hierophant proof request_id so we can look up
// worker status through the existing ProofRouter.
pub struct BonsaiState {
    pub inner: Mutex<BonsaiInner>,
}

pub struct BonsaiInner {
    // image_id (hex digest) -> ELF bytes
    pub images: HashMap<String, Vec<u8>>,
    // input uuid -> raw input bytes
    pub inputs: HashMap<String, Vec<u8>>,
    // session uuid -> session metadata
    pub sessions: HashMap<String, BonsaiSession>,
    // session uuid -> serialized receipt
    pub receipts: HashMap<String, Vec<u8>>,
    // snark uuid -> snark job metadata (backs POST /snark/create)
    pub snark_requests: HashMap<String, SnarkState>,
    // snark uuid -> wrapped receipt bytes (populated when the snark job's
    // underlying proof completes and the wrapped receipt is downloaded)
    pub snark_receipts: HashMap<String, Vec<u8>>,
}

pub struct BonsaiSession {
    pub request_id: B256,
    pub image_id: String,
    // Retained for auditability of which input produced this session even
    // though we don't currently look it up after session creation.
    #[allow(dead_code)]
    pub input_id: String,
    pub mode: Risc0ProofMode,
    // terminal error surfaced to GET /sessions/status/{uuid} on failure
    pub terminal_error: Option<String>,
}

pub struct SnarkState {
    // The session whose receipt we're wrapping. Retained for observability;
    // `request_id` is what we actually poll with.
    #[allow(dead_code)]
    pub session_id: String,
    // The hierophant request_id of the wrap job; used to poll the registry
    // for status and retrieve the wrapped receipt when complete.
    pub request_id: B256,
    // image_id of the underlying session, re-used to verify the wrapped
    // receipt against the same image once the wrap finishes.
    pub image_id: String,
    // Terminal error if the snark wrap can't be started or the wrap fails.
    pub terminal_error: Option<String>,
}

impl Default for BonsaiState {
    fn default() -> Self {
        Self::new()
    }
}

impl BonsaiState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(BonsaiInner {
                images: HashMap::new(),
                inputs: HashMap::new(),
                sessions: HashMap::new(),
                receipts: HashMap::new(),
                snark_requests: HashMap::new(),
                snark_receipts: HashMap::new(),
            }),
        }
    }
}
