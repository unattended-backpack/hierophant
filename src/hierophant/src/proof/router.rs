use crate::{
    artifact_store::{ArtifactStoreClient, ArtifactUri},
    proof::ProofStatus,
    worker_registry::WorkerRegistryClient,
};
use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use log::{error, warn};
use network_lib::{
    ContemplantProofRequest, Risc0ProofMode, Risc0ProofRequest, Sp1ProofRequest,
};
use sp1_sdk::{SP1Stdin, network::proto::network::ProofMode};
use tokio::time::Duration;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct ProofRouter {
    pub worker_registry_client: WorkerRegistryClient,
    pub mock_mode: bool,
    pub proof_status_timeout: Duration,
}

impl ProofRouter {
    pub fn new(config: &Config) -> Self {
        let worker_registry_client = WorkerRegistryClient::new(config.worker_registry.clone());

        Self {
            worker_registry_client,
            mock_mode: config.mock_mode,
            proof_status_timeout: config.worker_response_timeout_secs,
        }
    }

    // SP1 path: fetches the bincode-serialized SP1 ELF and SP1Stdin artifacts
    // out of the local store, assembles a Sp1ProofRequest, and hands it to the
    // worker registry for assignment.
    pub async fn route_sp1_proof(
        &self,
        request_id: B256,
        program_uri: ArtifactUri,
        stdin_uri: ArtifactUri,
        mode: ProofMode,
        artifact_store_client: ArtifactStoreClient,
    ) -> Result<()> {
        let stdin_artifact_bytes = match artifact_store_client
            .get_artifact_bytes(stdin_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(anyhow!("Stdin artifact with uri {stdin_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting stdin artifact {stdin_uri}: {e}")),
        };

        let sp1_stdin: SP1Stdin = bincode::deserialize(&stdin_artifact_bytes)?;

        let elf: Vec<u8> = match artifact_store_client
            .get_artifact_bytes(program_uri.clone())
            .await
        {
            Ok(Some(bytes)) => bincode::deserialize(&bytes)?,
            Ok(None) => return Err(anyhow!("Program artifact with uri {program_uri} not found")),
            Err(e) => return Err(anyhow!("Error getting program artifact {program_uri}: {e}")),
        };

        let proof_request = ContemplantProofRequest::Sp1(Sp1ProofRequest {
            request_id,
            mock: self.mock_mode,
            mode,
            sp1_stdin,
            elf,
        });

        self.worker_registry_client
            .assign_proof_request(proof_request)
            .await
    }

    // RISC Zero path: callers (the Bonsai REST handlers) provide the ELF bytes
    // and the opaque input blob directly, since Bonsai's wire protocol uploads
    // those as discrete resources rather than wrapped like SP1's do.
    //
    // `wrap_of` is for the two-step Bonsai `POST /snark/create` flow: when
    // Some, we're asking the contemplant to take an existing receipt (the
    // bytes) and wrap it into the requested `mode` (typically Groth16). In
    // that case `elf` and `input` are ignored by the contemplant and should
    // be passed as empty Vecs.
    pub async fn route_risc0_proof(
        &self,
        request_id: B256,
        elf: Vec<u8>,
        input: Vec<u8>,
        mode: Risc0ProofMode,
        wrap_of: Option<Vec<u8>>,
    ) -> Result<()> {
        let proof_request = ContemplantProofRequest::Risc0(Risc0ProofRequest {
            request_id,
            elf,
            input,
            mode,
            mock: self.mock_mode,
            wrap_of,
        });

        self.worker_registry_client
            .assign_proof_request(proof_request)
            .await
    }

    pub async fn get_proof_status(&self, proof_request_id: B256) -> Result<ProofStatus> {
        match self
            .worker_registry_client
            .proof_status_request(proof_request_id, self.proof_status_timeout)
            .await
        {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                warn!("Can't find proof request {proof_request_id}");
                Ok(ProofStatus::lost())
            }
            Err(e) => {
                error!("Can't get proof status of request {proof_request_id} from worker: {e}");
                Ok(ProofStatus::lost())
            }
        }
    }
}
