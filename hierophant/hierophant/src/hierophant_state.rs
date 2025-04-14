use crate::config::Config;
use crate::create_artifact_service::CreateArtifactService;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use alloy_primitives::{Address, B256};
use axum::body::Bytes;
use log::debug;
use serde::{Deserialize, Serialize};
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display},
    net::SocketAddr,
    sync::Arc,
};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Idle,
    Busy { proof_id: B256 },
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy { proof_id } => write!(f, "Busy with proof {proof_id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerState {
    pub name: String,
    pub id: B256,
    pub status: WorkerStatus,
    // publicly callable address
    pub address: SocketAddr,
    pub strikes: usize,
}

impl WorkerState {
    pub fn new(name: String, address: SocketAddr) -> WorkerState {
        let id = B256::random();
        WorkerState {
            name,
            id,
            address,
            status: WorkerStatus::Idle,
            strikes: 0,
        }
    }

    fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
    }

    fn add_strike(&mut self) {
        self.strikes += 1;
        debug!(
            "Strike added to worker {}:{}.  New strikes: {}",
            self.name, self.id, self.strikes
        );
    }

    fn add_strikes(&mut self, strikes: usize) {
        self.strikes += strikes;
        debug!(
            "{} strikes added to worker.  New strikes: {}",
            strikes, self.strikes
        );
    }

    // Makes the worker busy with a proof id
    fn assign_proof(&mut self, proof_id: B256) {
        self.status = WorkerStatus::Busy { proof_id };
        // This worker has been good.  Reset their strikes
        self.strikes = 0;
    }

    fn should_drop(&self, cfg_max_worker_strikes: usize) -> bool {
        self.strikes >= cfg_max_worker_strikes
    }

    // returns the proof the worker is currently working on, if any
    fn current_proof_id(&self) -> Option<B256> {
        match self.status {
            WorkerStatus::Idle => None,
            WorkerStatus::Busy { proof_id } => Some(proof_id),
        }
    }
}

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
    // Requested proofs
    pub proof_requests: Arc<Mutex<HashMap<Vec<u8>, ProofRequestData>>>,
    // mapping vk_hash -> Program
    // programs are requested by vk_hash in ProverNetworkService.get_program reqs
    pub program_store: Arc<Mutex<HashMap<Vec<u8>, Program>>>,
    pub nonces: Arc<Mutex<HashMap<Address, u64>>>,
    // mapping of artifact upload path to (expected type, uri)
    pub upload_urls: Arc<Mutex<HashMap<String, (ArtifactType, Uuid)>>>,
    // mapping of uri, artifact data
    pub artifact_store: Arc<Mutex<HashMap<Uuid, Artifact>>>,
}

impl HierophantState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            workers: Arc::new(RwLock::new(HashMap::new())),
            proof_requests: Arc::new(Mutex::new(HashMap::new())),
            program_store: Arc::new(Mutex::new(HashMap::new())),
            nonces: Arc::new(Mutex::new(HashMap::new())),
            upload_urls: Arc::new(Mutex::new(HashMap::new())),
            artifact_store: Arc::new(Mutex::new(HashMap::new())),
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
