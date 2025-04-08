use crate::config::Config;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerStatus {
    Available,
    Busy,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub name: String,
    pub id: String,
    pub status: WorkerStatus,
    pub last_heartbeat: u64,
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
    pub workers: Arc<Mutex<HashMap<String, WorkerInfo>>>,
    // Valid upload URLs
    pub upload_urls: Arc<Mutex<HashSet<String>>>,
    // Requested proofs
    pub proof_requests: Arc<Mutex<HashMap<Vec<u8>, ProofRequestData>>>,
}

impl HierophantState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            upload_urls: Arc::new(Mutex::new(HashSet::new())),
            workers: Arc::new(Mutex::new(HashMap::new())),
            proof_requests: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}
