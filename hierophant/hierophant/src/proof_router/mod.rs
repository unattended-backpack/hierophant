mod proof_cache;
mod worker_registry;

use crate::{
    artifact_store::{self, ArtifactStoreClient, ArtifactUri},
    hierophant_state::ProofStatus,
    network::RequestProofRequestBody,
};
use anyhow::{Context, Result, anyhow};
use log::{error, warn};
use network_lib::{ContemplantProofRequest, ProofRequestId};
use proof_cache::ProofCache;
use sp1_sdk::{
    SP1Stdin,
    network::proto::network::{ExecutionStatus, FulfillmentStatus, ProofMode},
};
use std::{fmt::Display, sync::Arc};
use tokio::{sync::Mutex, time::Instant};
use worker_registry::WorkerRegistryClient;
pub use worker_registry::WorkerState;

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
    pub async fn route_proof(
        &self,
        request_id: ProofRequestId,
        // uri of the ELF previously stored
        program_uri: ArtifactUri,
        // uri of the stdin previously stored
        stdin_uri: ArtifactUri,
        // Type of proof being requested
        mode: ProofMode,
        // Need to get Program and Stdin artifacts to request the proof, so we have to use the
        // artifact_store
        artifact_store_client: ArtifactStoreClient,
    ) -> Result<()> {
        // check if we have this proof on-disk cache, otherwise parse artifacts and send it to
        // the prover network
        if let Some(_) = self.proof_cache.lock().await.read_proof(&request_id) {
            // Don't route it.  We already have this proof on-disk in the proof cache.  It will be
            // retreived on get_proof_status
            return Ok(());
        };

        let stdin_artifact_bytes = match artifact_store_client
            .get_artifact_bytes(stdin_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(anyhow!("Stdin artifact with uri {stdin_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting stdin artifact {stdin_uri}: {e}")),
        };

        // TODO: where does witnessgen happen????
        let sp1_stdin: SP1Stdin = bincode::deserialize(&stdin_artifact_bytes)?;

        // get the elf
        let program_artifact_bytes = match artifact_store_client
            .get_artifact_bytes(program_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(anyhow!("Program artifact with uri {program_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting program artifact {program_uri}: {e}")),
        };

        let proof_request = ContemplantProofRequest {
            request_id,
            mock: self.mock_mode,
            mode,
            sp1_stdin,
            elf: program_artifact_bytes,
        };

        let res = self
            .worker_registry_client
            .assign_proof_request(request_id, proof_request)
            .await;
        res
    }

    // TODO: what if we get this request before the proof can be assigned to a worker?
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

        // then check the prover network
        match self
            .worker_registry_client
            .proof_status(proof_request_id)
            .await
        {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                warn!("Can't find proof request {proof_request_id}");
                Ok(ProofStatus::lost())
            }
            Err(e) => {
                // There's a worker assigned to this proof but we can't contact them
                // The worker likely went offline before they finished the proof
                warn!("Can't get proof status of request {proof_request_id} from worker: {e}");
                // TODO: is this the proper fulfil/exec status?  How does the client respond to
                // this
                Ok(ProofStatus::lost())
            }
        }
    }
}

// helper function to send requests to workers multiple times
pub async fn request_with_retries<F, Fut, T, E>(
    max_retries: usize,
    mut request_fn: F,
) -> Result<T, anyhow::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Display,
{
    let mut retry_num = 0;
    let mut last_error = None;

    while retry_num < max_retries {
        let request = request_fn();
        match request.await {
            Ok(res) => return Ok(res),
            Err(err) => {
                let error_msg = format!(
                    "Prover network request retry {}/{} failed: {}",
                    retry_num, max_retries, err
                );
                error!("{}", error_msg);

                last_error = Some(anyhow!("{}", err));
            }
        }
        retry_num += 1;
    }

    Err(anyhow!(
        "All {} requests to the prover network failed. Last error: {}",
        max_retries,
        last_error.unwrap_or_else(|| anyhow!("Unknown error"))
    ))
}
