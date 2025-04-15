mod proof_cache;
mod proof_request_cache;
mod worker_registry;

use anyhow::Context;
use proof_cache::ProofCache;
use proof_request_cache::ProofRequestCacheClient;
use std::sync::Arc;
use tokio::sync::RwLock;
use worker_registry::WorkerRegistryClient;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    // how many retries on prover network requests until we fall back to local proof.
    pub prover_network_retries: usize,
    // TODO: forward proof requests to the actual prover network.  This should be top-level config
    // tbh
    pub local_proving_only: bool,
    pub proof_cache: Arc<RwLock<ProofCache>>,
    pub worker_registry_client: WorkerRegistryClient,
    pub proof_request_cache_client: ProofRequestCacheClient,
    pub mock_mode: bool,
}

impl ProofRouter {
    // TODO: Should config live at the top level or is inside here okay?
    pub fn new(config: Config) -> Self {
        let proof_request_cache_client =
            ProofRequestCacheClient::new(config.proof_request_cache_size);
        let proof_cache = Arc::new(RwLock::new(
            ProofCache::new(
                config.proof_cache_size,
                &config.proof_cache_directory,
                proof_request_cache_client.clone(),
            )
            .context("Create proof cache")
            // This error is unrecoverable
            .unwrap(),
        ));
        let worker_registry_client =
            WorkerRegistryClient::new(config.max_worker_strikes, config.prover_network_retries);

        Self {
            prover_network_retries: config.prover_network_retries,
            local_proving_only: config.local_proving_only,
            proof_cache,
            worker_registry_client,
            proof_request_cache_client,
            mock_mode: config.mock_mode,
        }
    }
}
