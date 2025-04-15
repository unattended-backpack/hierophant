mod proof_cache;
mod worker_registry;

use anyhow::Context;
use proof_cache::ProofCache;
use std::sync::Arc;
use tokio::sync::RwLock;
use worker_registry::WorkerRegistryClient;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    // how many retries on prover network requests until we fall back to local proof.
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
}
