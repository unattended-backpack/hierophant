use crate::artifact_store::{ArtifactStoreClient, ArtifactUri};
use crate::config::Config;
use crate::network::{ExecutionStatus, FulfillmentStatus, Program, RequestProofRequestBody};
use crate::proof_router::ProofRouter;
use alloy_primitives::Address;
use network_lib::ProofRequestId;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display, hash::Hash, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct HierophantState {
    pub config: Config,
    // mapping id -> (proof_uri, ProofRequestBody)
    // TODO: If we only use this in the proof router, move it to ProofRouter state
    pub proof_requests: Arc<Mutex<HashMap<ProofRequestId, (ArtifactUri, RequestProofRequestBody)>>>,
    // mapping vk_hash -> Program (contains program_uri)
    // programs are requested by vk_hash in ProverNetworkService.get_program reqs
    pub program_store: Arc<Mutex<HashMap<VkHash, Program>>>,
    // mapping of artifact upload path to (expected type, uri)
    pub artifact_store_client: ArtifactStoreClient,
    // handles delegating proof requests to contemplants and monitoring their progress
    pub proof_router: ProofRouter,
    // TODO: use (lol)
    pub nonces: Arc<Mutex<HashMap<Address, u64>>>,
}

impl HierophantState {
    pub fn new(config: Config) -> Self {
        let proof_router = ProofRouter::new(&config);
        let artifact_store_client = ArtifactStoreClient::new(&config.artifact_store_directory);
        Self {
            config,
            proof_requests: Arc::new(Mutex::new(HashMap::new())),
            program_store: Arc::new(Mutex::new(HashMap::new())),
            nonces: Arc::new(Mutex::new(HashMap::new())),
            artifact_store_client,
            proof_router,
        }
    }
}

// newtype wrapper for keeping vk_hash bytes distinct from other Vec<u8>
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct VkHash(Vec<u8>);

impl VkHash {
    pub fn to_hex_string(&self) -> String {
        format!("0x{}", hex::encode(self.clone().0))
    }
}

impl Default for VkHash {
    fn default() -> Self {
        VkHash(vec![])
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

/*
pub enum FulfillmentStatus {
    UnspecifiedFulfillmentStatus = 0,
    /// Proof request is pending
    Pending = 1,
    /// Proof request is assigned to a prover
    Assigned = 2,
    /// Proof has been generated
    Fulfilled = 3,
    /// Proof generation failed
    Failed = 4,
    /// Proof request was cancelled
    Cancelled = 5,
}

pub enum ExecutionStatus {
    UnspecifiedExecutionStatus = 0,
    /// Execution is pending
    Unexecuted = 1,
    /// Execution completed successfully
    Executed = 2,
    /// Execution failed
    Unexecutable = 3,
}
*/

#[derive(Serialize, Deserialize, Debug)]
/// The status of a proof request.
pub struct ProofStatus {
    // Note: Can't use `FulfillmentStatus`/`ExecutionStatus` directly because `Serialize_repr` and `Deserialize_repr` aren't derived on it.
    pub fulfillment_status: i32,
    pub execution_status: i32,
    pub proof: Vec<u8>,
}

impl ProofStatus {
    pub fn lost() -> Self {
        Self {
            fulfillment_status: FulfillmentStatus::Unfulfillable.into(),
            execution_status: ExecutionStatus::Unexecutable.into(),
            proof: vec![],
        }
    }
}

impl Display for ProofStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fulfillment_status = FulfillmentStatus::try_from(self.fulfillment_status)
            .unwrap_or(FulfillmentStatus::UnspecifiedFulfillmentStatus)
            .as_str_name();

        let execution_status = ExecutionStatus::try_from(self.execution_status)
            .unwrap_or(ExecutionStatus::UnspecifiedExecutionStatus)
            .as_str_name();

        let proof_display = if self.proof.is_empty() {
            "Empty"
        } else {
            "Non-empty"
        };

        write!(
            f,
            "FulfillmentStatus: {}, ExecutionStatus: {}, Proof: {}",
            fulfillment_status, execution_status, proof_display
        )
    }
}
