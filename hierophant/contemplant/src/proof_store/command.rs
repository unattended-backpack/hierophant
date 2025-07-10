use alloy_primitives::B256;
use network_lib::{ContemplantProofStatus, ProgressUpdate};
use std::fmt;
use tokio::sync::oneshot;

pub(super) enum ProofStoreCommand {
    GetProofStatus {
        request_id: B256,
        resp_sender: oneshot::Sender<Option<ContemplantProofStatus>>,
    },
    ProofProgressUpdate {
        request_id: B256,
        update: ProgressUpdate,
    },
    ProofStatusUpdate {
        request_id: B256,
        execution_status: i32,
        proof: Option<Vec<u8>>,
    },
    InsertProof {
        request_id: B256,
        proof_status: ContemplantProofStatus,
    },
}

impl fmt::Debug for ProofStoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let command = match self {
            ProofStoreCommand::GetProofStatus { .. } => {
                format!("GetProofStatus")
            }
            ProofStoreCommand::ProofProgressUpdate { .. } => {
                format!("ProofProgressUpdate")
            }
            ProofStoreCommand::ProofStatusUpdate { .. } => {
                format!("ProofStatusUpdate")
            }
            ProofStoreCommand::InsertProof { .. } => {
                format!("InsertProof")
            }
        };
        write!(f, "{command}")
    }
}
