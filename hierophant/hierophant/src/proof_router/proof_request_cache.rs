use anyhow::{Context, Result, anyhow};
use log::{error, info};
use std::collections::HashMap;

use crate::GenericProofRequest;
use alloy_primitives::B256;
use tokio::sync::{mpsc, oneshot};

// TODO: maybe don't need to make this a cache
#[derive(Clone, Debug)]
pub struct ProofRequestCacheClient {
    sender: mpsc::Sender<ProofRequestCacheCommand>,
}

impl ProofRequestCacheClient {
    pub fn new(max_proof_requests_stored: usize) -> Self {
        let (sender, receiver) = mpsc::channel(100);

        let cache_size = max_proof_requests_stored;
        let mut cache_list = Vec::with_capacity(cache_size);
        // fill with default values so we never have to check if current_cache_index is out of
        // bounds
        cache_list.resize_with(cache_size, || B256::default());

        let proof_request_cache = ProofRequestCache {
            cache_size: max_proof_requests_stored,
            cache_list,
            current_cache_index: 0,
            proof_requests: HashMap::new(),
            receiver,
        };

        tokio::task::spawn(async move { proof_request_cache.background_event_loop().await });

        Self { sender }
    }

    pub async fn record_proof_request(
        &self,
        proof_id: B256,
        proof_request: GenericProofRequest,
    ) -> Result<()> {
        self.sender
            .send(ProofRequestCacheCommand::RecordProofRequest {
                proof_id,
                proof_request,
            })
            .await?;

        Ok(())
    }

    pub async fn lookup_proof_request(
        &self,
        proof_id: &B256,
    ) -> Result<Option<GenericProofRequest>> {
        let (response_sender, receiver) = oneshot::channel();
        self.sender
            .send(ProofRequestCacheCommand::LookupProof {
                proof_id: *proof_id,
                response_sender,
            })
            .await?;
        receiver.await.map_err(|e| anyhow!(e))
    }
}

struct ProofRequestCache {
    proof_requests: HashMap<B256, GenericProofRequest>,
    receiver: mpsc::Receiver<ProofRequestCacheCommand>,
    current_cache_index: usize,
    // max proof requests that can be stored
    cache_size: usize,
    cache_list: Vec<B256>,
}

impl ProofRequestCache {
    async fn background_event_loop(mut self) {
        while let Some(command) = self.receiver.recv().await {
            match command {
                ProofRequestCacheCommand::RecordProofRequest {
                    proof_id,
                    proof_request,
                } => {
                    self.handle_record_proof_request(proof_id, proof_request);
                }
                ProofRequestCacheCommand::LookupProof {
                    proof_id,
                    response_sender,
                } => {
                    self.handle_lookup_proof(proof_id, response_sender);
                }
            }
        }
    }

    fn handle_record_proof_request(&mut self, proof_id: B256, proof_request: GenericProofRequest) {
        // overwrite the current_cache_index of cache_list
        let old_proof_id = match self.cache_list.get_mut(self.current_cache_index) {
            Some(elem) => elem,
            None => {
                error!(
                    "index {} out of bounds of proof request cache list",
                    self.current_cache_index
                );
                // TODO: maybe we should panic exit here?  Quite severe
                return;
            }
        };

        // remove old proof from the mapping
        self.proof_requests.remove_entry(old_proof_id);

        // add our new proof to the mapping
        self.proof_requests.insert(proof_id, proof_request);

        // overwrite the current_cache_index in the cache_list vector with the new entry
        *old_proof_id = proof_id;

        self.increment_current_cache_index();
    }

    fn increment_current_cache_index(&mut self) {
        if self.cache_size > 0 {
            // increment by 1, looping to the start if its at capacity (cache_size)
            let new_cache_index = (self.current_cache_index + 1) % self.cache_size;
            self.current_cache_index = new_cache_index;
        }
    }

    fn handle_lookup_proof(
        &self,
        proof_id: B256,
        response_sender: oneshot::Sender<Option<GenericProofRequest>>,
    ) {
        let maybe_proof_request = self.proof_requests.get(&proof_id);
        response_sender.send(maybe_proof_request.cloned());
    }
}

enum ProofRequestCacheCommand {
    RecordProofRequest {
        proof_id: B256,
        proof_request: GenericProofRequest,
    },
    LookupProof {
        proof_id: B256,
        response_sender: oneshot::Sender<Option<GenericProofRequest>>,
    },
}
