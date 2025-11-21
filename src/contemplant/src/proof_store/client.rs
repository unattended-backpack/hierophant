use alloy_primitives::B256;
use log::error;
use network_lib::{ContemplantProofStatus, ProgressUpdate};
use tokio::sync::{mpsc, oneshot};

use super::command::ProofStoreCommand;
use super::store::ProofStore;

// public interface for interacting with the proof store
#[derive(Clone)]
pub struct ProofStoreClient {
    sender: mpsc::Sender<ProofStoreCommand>,
}

impl ProofStoreClient {
    pub fn new(max_proofs_stored: usize) -> Self {
        if max_proofs_stored < 1 {
            let error_msg = "Contemplant's config max_proofs_stored must be > 1";
            error!("{error_msg}");
            panic!("{error_msg}");
        }
        // initialize proofs to be size <max_proofs_stored>
        let default_proof = (B256::default(), ContemplantProofStatus::default());
        let proofs = vec![default_proof; max_proofs_stored];

        let (sender, receiver) = mpsc::channel(100);

        let store = ProofStore::new(max_proofs_stored, proofs, 0, receiver);

        tokio::task::spawn(async move { store.background_event_loop().await });

        ProofStoreClient { sender }
    }

    pub async fn insert(&self, request_id: B256, proof_status: ContemplantProofStatus) {
        let command = ProofStoreCommand::InsertProof {
            request_id,
            proof_status,
        };

        if let Err(e) = self.sender.send(command).await {
            error!("Failed to send command InsertProof: {e}");
        }
    }

    pub async fn get(&self, request_id: B256) -> Option<ContemplantProofStatus> {
        let (resp_sender, resp_receiver) = oneshot::channel();
        let command = ProofStoreCommand::GetProofStatus {
            request_id,
            resp_sender,
        };

        if let Err(e) = self.sender.send(command).await {
            error!("Failed to send command GetProofStatus: {e}");
        }

        match resp_receiver.await {
            Ok(maybe_proof_status) => maybe_proof_status,
            Err(e) => {
                error!("Error getting proof status for {request_id}: {e}");
                None
            }
        }
    }

    pub async fn proof_progress_update(&self, request_id: B256, update: ProgressUpdate) {
        let command = ProofStoreCommand::ProofProgressUpdate { request_id, update };
        if let Err(e) = self.sender.send(command).await {
            error!("Failed to send command ProofProgressUpdate: {e}");
        }
    }

    pub async fn proof_status_update(
        &self,
        request_id: B256,
        execution_status: i32,
        proof: Option<Vec<u8>>,
    ) {
        let command = ProofStoreCommand::ProofStatusUpdate {
            request_id,
            execution_status,
            proof,
        };
        if let Err(e) = self.sender.send(command).await {
            error!("Failed to send command ProofStatusUpdate: {e}");
        }
    }
}
