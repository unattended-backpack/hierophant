mod worker_registry;

use crate::{
    artifact_store::{ArtifactStoreClient, ArtifactUri},
    hierophant_state::ProofStatus,
};
use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use log::{error, warn};
use network_lib::ContemplantProofRequest;
use sp1_sdk::{SP1Stdin, network::proto::network::ProofMode};
use std::fmt::Display;
use worker_registry::WorkerRegistryClient;
pub use worker_registry::WorkerState;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    pub worker_registry_client: WorkerRegistryClient,
    pub mock_mode: bool,
}

impl ProofRouter {
    // TODO: Should config live at the top level or is inside here okay?
    pub fn new(config: &Config) -> Self {
        let worker_registry_client = WorkerRegistryClient::new(config.max_worker_strikes);

        Self {
            worker_registry_client,
            mock_mode: config.mock_mode,
        }
    }

    // looks on-disk for the proof, checks for contemplants currently working on the proof,
    // or routes the proof request to an idle contemplant.
    // returns a proof request id
    pub async fn route_proof(
        &self,
        request_id: B256,
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
        // TODO: first check if we have it in artifact_store already

        let stdin_artifact_bytes = match artifact_store_client
            .get_artifact_bytes(stdin_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(anyhow!("Stdin artifact with uri {stdin_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting stdin artifact {stdin_uri}: {e}")),
        };

        let sp1_stdin: SP1Stdin = bincode::deserialize(&stdin_artifact_bytes)?;

        // get the elf
        let elf = match artifact_store_client
            .get_artifact_bytes(program_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bincode::deserialize(&bytes)?,
            Ok(None) => return Err(anyhow!("Program artifact with uri {program_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting program artifact {program_uri}: {e}")),
        };

        let proof_request = ContemplantProofRequest {
            request_id,
            mock: self.mock_mode,
            mode,
            sp1_stdin,
            elf,
        };

        let res = self
            .worker_registry_client
            .assign_proof_request(request_id, proof_request)
            .await;
        res
    }

    pub async fn get_proof_status(&self, proof_request_id: B256) -> Result<ProofStatus> {
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
