use super::command::ProofStoreCommand;
use alloy_primitives::B256;
use log::{info, trace, warn};
use network_lib::{ContemplantProofStatus, ProgressUpdate};
use tokio::{sync::mpsc, time::Instant};

// ProofStore holds all of the state and receives commands to store or return data

// datastructure to store only <max_proofs_stored> proofs because they can be very big an expensive
// to store in memory.  In most cases, we only need to stored the most recent proof in memory, but
// there are some edge cases where the last 2-3 are needed.
pub(super) struct ProofStore {
    max_proofs_stored: usize,
    proofs: Vec<(B256, ContemplantProofStatus)>,
    current_proof_index: usize,
    receiver: mpsc::Receiver<ProofStoreCommand>,
}

impl ProofStore {
    pub(super) fn new(
        max_proofs_stored: usize,
        proofs: Vec<(B256, ContemplantProofStatus)>,
        current_proof_index: usize,
        receiver: mpsc::Receiver<ProofStoreCommand>,
    ) -> ProofStore {
        Self {
            max_proofs_stored,
            proofs,
            current_proof_index,
            receiver,
        }
    }

    pub(super) async fn background_event_loop(mut self) {
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
                    let proof = self
                        .proofs
                        .iter()
                        .find(|(this_request_id, _)| *this_request_id == request_id)
                        .map(|(_, proof_status)| proof_status.clone());

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
                            if proof.is_some() {
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
