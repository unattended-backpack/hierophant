use crate::artifact_store::{ArtifactStoreClient, ArtifactUri};
use crate::config::Config;
use crate::network::{Program, RequestProofRequestBody};
use crate::proof::ProofRouter;
use alloy_primitives::{Address, B256};
use anyhow::anyhow;
use serde::Serialize;
use sp1_sdk::{CpuProver, SP1VerifyingKey};
use std::{collections::HashMap, hash::Hash, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct HierophantState {
    pub config: Config,
    // mapping id -> (proof_uri, ProofRequestBody)
    // TODO: If we only use this in the proof router, move it to ProofRouter state
    pub proof_requests: Arc<Mutex<HashMap<B256, (ArtifactUri, RequestProofRequestBody)>>>,
    // mapping vk_hash -> Program (contains program_uri)
    // programs are requested by vk_hash in ProverNetworkService.get_program reqs
    pub program_store: Arc<Mutex<HashMap<VkHash, Program>>>,
    // mapping of artifact upload path to (expected type, uri)
    pub artifact_store_client: ArtifactStoreClient,
    // handles delegating proof requests to contemplants and monitoring their progress
    pub proof_router: ProofRouter,
    // solely used for verifying proofs returned from contemplant
    pub cpu_prover: Arc<CpuProver>,
    // TODO: use (lol)
    pub nonces: Arc<Mutex<HashMap<Address, u64>>>,
}

impl HierophantState {
    pub fn new(config: Config) -> Self {
        let proof_router = ProofRouter::new(&config);
        let artifact_store_client = ArtifactStoreClient::new(
            &config.artifact_store_directory,
            config.max_stdin_artifacts_stored,
            config.max_proof_artifacts_stored,
        );
        let cpu_prover = Arc::new(CpuProver::new());
        Self {
            config,
            proof_requests: Arc::new(Mutex::new(HashMap::new())),
            program_store: Arc::new(Mutex::new(HashMap::new())),
            nonces: Arc::new(Mutex::new(HashMap::new())),
            artifact_store_client,
            proof_router,
            cpu_prover,
        }
    }

    // convenience function used in prover_network_service.get_proof_request_status.
    // Needed to verify proof
    pub async fn get_vk(&self, request_id: &B256) -> anyhow::Result<SP1VerifyingKey> {
        // we have the proof request_id and we need to get to the vkey
        let vk_hash: VkHash = match self.proof_requests.lock().await.get(request_id) {
            Some((_, request_body)) => request_body.vk_hash.clone().into(),
            None => {
                return Err(anyhow!(
                    "Can't find proof request {request_id} when looking up vk_hash"
                ));
            }
        };

        let vk_bytes = match self.program_store.lock().await.get(&vk_hash) {
            Some(program) => program.vk.clone(),
            None => {
                return Err(anyhow!(
                    "Can't find program with vk_hash {}",
                    vk_hash.to_hex_string()
                ));
            }
        };

        bincode::deserialize(&vk_bytes).map_err(|e| anyhow!(e))
    }
}

// newtype wrapper for keeping vk_hash bytes distinct from other Vec<u8>
#[derive(Default, Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct VkHash(Vec<u8>);

impl VkHash {
    pub fn to_hex_string(&self) -> String {
        format!("0x{}", hex::encode(self.clone().0))
    }
}

// so we can convert from Vec<u8> to VkHash
impl From<Vec<u8>> for VkHash {
    fn from(bytes: Vec<u8>) -> Self {
        VkHash(bytes)
    }
}

// so we can convert VkHash into Vec<u8>
impl From<VkHash> for Vec<u8> {
    fn from(hash: VkHash) -> Self {
        hash.0
    }
}
