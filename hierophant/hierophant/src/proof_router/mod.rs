mod proof_cache;
mod worker_registry;
pub mod worker_state;

use crate::hierophant_state::{ProofRequestId, ProofStatus};
use anyhow::{Context, Result};
use proof_cache::ProofCache;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;
use worker_registry::WorkerRegistryClient;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    pub proof_cache: Arc<RwLock<ProofCache>>,
    pub worker_registry_client: WorkerRegistryClient,
    pub mock_mode: bool,
}

impl ProofRouter {
    // TODO: Should config live at the top level or is inside here okay?
    pub fn new(config: &Config) -> Self {
        let proof_cache = Arc::new(RwLock::new(
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
    pub fn route_proof(&self, proof_request: ProofRequestId) -> todo!() {
        todo!()
    }

    pub async fn get_proof_status(&self, proof_request_id: ProofRequestId) -> Result<ProofStatus> {
        // fulfillment_status wil be tracked in ProofRouter
        // The contemplant will only ever return execution_status and the proof (if it's done)
        todo!()
    }
}
