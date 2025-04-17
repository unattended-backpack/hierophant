mod proof_cache;
mod worker_registry;
pub mod worker_state;

use crate::hierophant_state::ProofStatus;
use anyhow::{Context, Result};
use network_lib::ProofRequestId;
use proof_cache::ProofCache;
use sp1_sdk::network::proto::network::{ExecutionStatus, FulfillmentStatus};
use std::sync::Arc;
use tokio::sync::Mutex;
use worker_registry::WorkerRegistryClient;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    pub proof_cache: Arc<Mutex<ProofCache>>,
    pub worker_registry_client: WorkerRegistryClient,
    pub mock_mode: bool,
}

impl ProofRouter {
    // TODO: Should config live at the top level or is inside here okay?
    pub fn new(config: &Config) -> Self {
        let proof_cache = Arc::new(Mutex::new(
            ProofCache::new(config.proof_cache_size, &config.proof_cache_directory)
                .context("Create proof cache")
                // This error is unrecoverable
                .unwrap(),
        ));
        let worker_registry_client = WorkerRegistryClient::new(config.max_worker_strikes);

        Self {
            proof_cache,
            worker_registry_client,
            mock_mode: config.mock_mode,
        }
    }

    // looks on-disk for the proof, checks for contemplants currently working on the proof,
    // or routes the proof request to an idle contemplant.
    // returns a proof request id
    pub async fn route_proof(&self, proof_request_id: ProofRequestId) -> Result<()> {
        if let Some(_) = self.proof_cache.lock().await.read_proof(&proof_request_id) {
            // Don't route it.  We already have this proof on-disk in the proof cache.  It will be
            // retreived on get_proof_status
            return Ok(());
        };

        // otherwise send it to our prover network
        todo!()
    }

    pub async fn get_proof_status(&self, proof_request_id: ProofRequestId) -> Result<ProofStatus> {
        // first check to see if we have it in the proof cache
        if let Some(proof_bytes) = self.proof_cache.lock().await.read_proof(&proof_request_id) {
            let status = ProofStatus {
                fulfillment_status: FulfillmentStatus::Fulfilled.into(),
                execution_status: ExecutionStatus::Executed.into(),
                proof: proof_bytes,
            };

            return Ok(status);
        };

        // fulfillment_status wil be tracked in ProofRouter
        // The contemplant will only ever return execution_status and the proof (if it's done)
        todo!()
    }
}
