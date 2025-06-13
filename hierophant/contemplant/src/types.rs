use alloy_primitives::B256;
use log::error;
use log::{info, trace, warn};
use network_lib::{ContemplantProofStatus, ProgressUpdate};
use std::fmt;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

#[derive(Clone)]
pub struct ProofStoreClient {
    sender: mpsc::Sender<ProofStoreCommand>,
}

impl ProofStoreClient {
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

// datastructure to store only <max_proofs_stored> proofs because they can be very big an expensive
// to store in memory.  In most cases, we only need to stored the most recent proof in memory, but
// there are some edge cases where the last 2-3 are needed.
pub struct ProofStore {
    max_proofs_stored: usize,
    proofs: Vec<(B256, ContemplantProofStatus)>,
    current_proof_index: usize,
    receiver: mpsc::Receiver<ProofStoreCommand>,
}

impl ProofStore {
    pub fn new(max_proofs_stored: usize) -> ProofStoreClient {
        if max_proofs_stored < 1 {
            let error_msg = format!("Contemplant's config max_proofs_stored must be > 1");
            error!("{error_msg}");
            panic!("{error_msg}");
        }
        // initialize proofs to be size <max_proofs_stored>
        let default_proof = (B256::default(), ContemplantProofStatus::default());
        let proofs = vec![default_proof; max_proofs_stored];

        let (sender, receiver) = mpsc::channel(100);

        let store = Self {
            max_proofs_stored,
            proofs,
            current_proof_index: 0,
            receiver,
        };

        tokio::task::spawn(async move { store.background_event_loop().await });

        ProofStoreClient { sender }
    }

    async fn background_event_loop(mut self) {
        while let Some(command) = self.receiver.recv().await {
            let start = Instant::now();
            let command_string = format!("{:?}", command);
            trace!(
                "{} messages in worker registry channel",
                self.receiver.len()
            );
            match command {
                ProofStoreCommand::GetProofStatus {
                    request_id,
                    resp_sender,
                } => {
                    // yes I know, this is O(n) when it could be O(1) with a hash map BUT in practice
                    // it is much faster because self.proofs.length is < 5 and we don't
                    // have to hash anything
                    let proof = match self
                        .proofs
                        .iter()
                        .find(|(this_request_id, _)| *this_request_id == request_id)
                    {
                        Some((_, proof_status)) => Some(proof_status.clone()),
                        None => None,
                    };

                    let _ = resp_sender.send(proof);
                }
                ProofStoreCommand::ProofProgressUpdate { request_id, update } => {
                    // yes I know, this is O(n) when it could be O(1) with a hash map BUT in practice
                    // it is much faster because self.proofs.length is < 5 and we don't
                    // have to hash anything
                    match self
                        .proofs
                        .iter_mut()
                        .find(|(this_request_id, _)| *this_request_id == request_id)
                    {
                        Some((_, proof_status)) => {
                            proof_status.progress_update(Some(update));
                        }
                        None => {
                            warn!(
                                "No proof with request id {request_id} was found for a progress update"
                            );
                        }
                    };
                }
                ProofStoreCommand::ProofStatusUpdate {
                    request_id,
                    execution_status,
                    proof,
                } => {
                    // yes I know, this is O(n) when it could be O(1) with a hash map BUT in practice
                    // it is much faster because self.proofs.length is < 5 and we don't
                    // have to hash anything
                    match self
                        .proofs
                        .iter_mut()
                        .find(|(this_request_id, _)| *this_request_id == request_id)
                    {
                        Some((_, proof_status)) => {
                            if let Some(_) = &proof {
                                proof_status.progress = Some(ProgressUpdate::Done);
                            }
                            proof_status.execution_status = execution_status;
                            proof_status.proof = proof;
                        }
                        None => {
                            warn!(
                                "No proof with request id {request_id} was found for a status update"
                            );
                        }
                    };
                }
                ProofStoreCommand::InsertProof {
                    request_id,
                    proof_status,
                } => {
                    // overwrite the current index with the new proof status, then increment the current index
                    match self
                        .proofs
                        .iter()
                        .enumerate()
                        .find(|(_, (this_request_id, _))| this_request_id == &request_id)
                    {
                        // if this request_id is already in the list, overwrite its index
                        Some((index, _)) => self.proofs[index] = (request_id, proof_status),
                        None => {
                            self.proofs[self.current_proof_index] = (request_id, proof_status);
                            self.increment_current_proof_index();
                        }
                    }
                }
            }

            let secs = start.elapsed().as_secs_f64();

            if secs > 0.5 {
                info!(
                    "Slow execution detected: took {} seconds to process worker_registry command {:?}",
                    secs, command_string
                );
            }
        }
    }

    // increments the next index to insert a proof by 1, looping back to the start of the vector if
    // we're at the end
    fn increment_current_proof_index(&mut self) {
        // increment by 1, looping to the start if its at capacity (proof_size)
        let new_proof_index = (self.current_proof_index + 1) % self.max_proofs_stored;
        self.current_proof_index = new_proof_index;
    }
}

pub enum ProofStoreCommand {
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

#[cfg(test)]
mod tests {
    //use super::*;

    // fn generate_proofs(x: usize) -> Vec<(B256, ContemplantProofStatus)> {
    //     let mut proofs = Vec::new();
    //     for _ in 0..x {
    //         proofs.push((B256::random(), ContemplantProofStatus::unexecuted()));
    //     }
    //     proofs
    // }

    /*
    #[test]
    fn test_init_proof_store() {
        let store = ProofStore::new(1);
        assert_eq!(store.proofs.len(), 1);

        let store = ProofStore::new(50);
        assert_eq!(store.proofs.len(), 50);
    }

    #[test]
    fn test_insert_proof() {
        let proof = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        store.insert(proof.0, proof.1.clone());

        let proof_status = store.get(&proof.0).cloned();

        assert_eq!(proof_status, Some(proof.1));

        for new_proof in generate_proofs(2) {
            store.insert(new_proof.0, new_proof.1.clone());
        }

        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, None);
    }

    #[test]
    fn test_increment_index() {
        let proof = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        assert_eq!(store.current_proof_index, 0);
        store.insert(proof.0, proof.1.clone());
        assert_eq!(store.current_proof_index, 1);
        // overwrite that proof status
        store.insert(proof.0, ContemplantProofStatus::default());
        assert_eq!(store.current_proof_index, 1);

        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, Some(ContemplantProofStatus::default()));

        for new_proof in generate_proofs(3) {
            store.insert(new_proof.0, new_proof.1.clone());
        }

        assert_eq!(store.current_proof_index, 0);

        // previous proof doesnt exist anymore
        let proof_status = store.get(&proof.0).cloned();
        assert_eq!(proof_status, None);
    }

    #[test]
    fn test_realistic_insert_pattern() {
        let proof_a = (B256::random(), ContemplantProofStatus::unexecuted());
        let proof_a_executed = (
            proof_a.0,
            ContemplantProofStatus::proof_complete(vec![], 12),
        );

        let proof_b = (B256::random(), ContemplantProofStatus::unexecuted());
        let proof_b_executed = (
            proof_b.0,
            ContemplantProofStatus::proof_complete(vec![], 12),
        );

        let proof_c = (B256::random(), ContemplantProofStatus::unexecuted());

        let mut store = ProofStore::new(2);
        assert_eq!(store.current_proof_index, 0);

        store.insert(proof_a.0, proof_a.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_a.0), Some(&proof_a.1));

        store.insert(proof_a_executed.0, proof_a_executed.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_b.0, proof_b.1.clone());
        assert_eq!(store.current_proof_index, 0);
        assert_eq!(store.get(&proof_b.0), Some(&proof_b.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_b_executed.0, proof_b_executed.1.clone());
        assert_eq!(store.current_proof_index, 0);
        assert_eq!(store.get(&proof_b_executed.0), Some(&proof_b_executed.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_a_executed.0), Some(&proof_a_executed.1));

        store.insert(proof_c.0, proof_c.1.clone());
        assert_eq!(store.current_proof_index, 1);
        assert_eq!(store.get(&proof_c.0), Some(&proof_c.1));

        // we should still be able to get the previous proof
        assert_eq!(store.get(&proof_b_executed.0), Some(&proof_b_executed.1));

        // but proof_a should have been pushed out by now
        assert_eq!(store.get(&proof_a_executed.0), None);
    }
    */
}
