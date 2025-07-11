use alloy_primitives::B256;
use serde::Serialize;
use sp1_sdk::network::proto::network::ProofMode;

#[derive(Serialize, Debug, Clone)]
pub struct CompletedProofInfo {
    proof_request_id: B256,
    proof_mode: String,
    worker_addr: String,
    worker_name: String,
    minutes_to_complete: f32,
}

impl CompletedProofInfo {
    pub fn new(
        proof_request_id: B256,
        proof_mode: ProofMode,
        minutes_to_complete: f32,
        worker_addr: String,
        worker_name: String,
    ) -> Self {
        Self {
            proof_request_id,
            proof_mode: proof_mode.as_str_name().into(),
            minutes_to_complete,
            worker_name,
            worker_addr,
        }
    }
}
