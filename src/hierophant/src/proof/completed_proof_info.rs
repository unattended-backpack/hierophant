use alloy_primitives::B256;
use network_lib::VmKind;
use serde::Serialize;

#[derive(Serialize, Debug, Clone)]
pub struct CompletedProofInfo {
    proof_request_id: B256,
    vm: VmKind,
    proof_mode: String,
    worker_addr: String,
    worker_name: String,
    minutes_to_complete: f32,
}

impl CompletedProofInfo {
    pub fn new(
        proof_request_id: B256,
        vm: VmKind,
        proof_mode: String,
        minutes_to_complete: f32,
        worker_addr: String,
        worker_name: String,
    ) -> Self {
        Self {
            proof_request_id,
            vm,
            proof_mode,
            minutes_to_complete,
            worker_name,
            worker_addr,
        }
    }
}
