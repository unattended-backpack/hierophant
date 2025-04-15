use crate::config::Config;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use crate::proof_router::{ProofRouter, worker_state::WorkerState};
use alloy_primitives::{Address, B256};
use axum::body::Bytes;
use log::debug;
use serde::{Deserialize, Serialize};
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display},
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

// Structure to store proof request data for status checks
#[derive(Debug, Clone)]
pub struct ProofRequestData {
    pub tx_hash: Vec<u8>,
    pub deadline: u64,
    pub fulfillment_status: FulfillmentStatus,
    pub execution_status: ExecutionStatus,
    pub proof_uri: Option<String>,
    pub fulfill_tx_hash: Option<Vec<u8>>,
    pub public_values_hash: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct HierophantState {
    pub config: Config,
    // Registered workers
    pub workers: Arc<RwLock<HashMap<String, WorkerState>>>,
    // mapping vk_hash -> ProofRequestData
    // Requested proofs
    pub proof_requests: Arc<Mutex<HashMap<VkHash, ProofRequestData>>>,
    // mapping vk_hash -> Program
    // programs are requested by vk_hash in ProverNetworkService.get_program reqs
    pub program_store: Arc<Mutex<HashMap<VkHash, Program>>>,
    pub nonces: Arc<Mutex<HashMap<Address, u64>>>,
    // mapping of artifact upload path to (expected type, uri)
    pub upload_urls: Arc<Mutex<HashMap<String, (ArtifactType, Uuid)>>>,
    // mapping of uri, artifact data
    pub artifact_store: Arc<Mutex<HashMap<Uuid, Artifact>>>,
    pub proof_router: ProofRouter,
}

impl HierophantState {
    pub fn new(config: Config) -> Self {
        let proof_router = ProofRouter::new(&config);
        Self {
            config,
            workers: Arc::new(RwLock::new(HashMap::new())),
            proof_requests: Arc::new(Mutex::new(HashMap::new())),
            program_store: Arc::new(Mutex::new(HashMap::new())),
            nonces: Arc::new(Mutex::new(HashMap::new())),
            upload_urls: Arc::new(Mutex::new(HashMap::new())),
            artifact_store: Arc::new(Mutex::new(HashMap::new())),
            proof_router,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Artifact {
    pub artifact_type: ArtifactType,
    // serialized representation of the artifact
    pub bytes: Bytes,
}

impl Artifact {
    pub fn new(artifact_type: ArtifactType, bytes: Bytes) -> Self {
        Self {
            artifact_type,
            bytes,
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
