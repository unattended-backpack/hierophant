use super::worker_state::WorkerState;
use crate::proof::CompletedProofInfo;
use alloy_primitives::B256;
use network_lib::{
    ContemplantProofRequest, ContemplantProofStatus, ProgressUpdate,
    messages::FromHierophantMessage,
};
use std::fmt::{self};
use tokio::sync::{mpsc, oneshot};

pub enum WorkerRegistryCommand {
    AssignProofRequest {
        proof_request: ContemplantProofRequest,
    },
    WorkerReady {
        worker_addr: String,
        worker_name: String,
        magister_drop_endpoint: Option<String>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    },
    // sp1_sdk requests the status of a proof
    ProofStatusRequest {
        target_request_id: B256,
        resp_sender: oneshot::Sender<Option<ContemplantProofStatus>>,
    },
    // a contemplant responds with a previously requested proof status
    ProofStatusResponse {
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    },
    ProofComplete {
        request_id: B256,
    },
    ProofProgressUpdate {
        request_id: B256,
        progress_update: Option<ProgressUpdate>,
    },
    Workers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
    DeadWorkers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
    ProofHistory {
        resp_sender: oneshot::Sender<Vec<CompletedProofInfo>>,
    },
    Heartbeat {
        worker_addr: String,
        should_drop_sender: oneshot::Sender<bool>,
    },
    // only used in external (external to workerRegistry state) functions like
    // WorkerRegistryClient.proof_status_request
    StrikeWorkerOfRequest {
        request_id: B256,
    },
    // only used in external (external to workerRegistry state) functions like
    // prover_network_service.get_proof_status (drop worker when they return an invalid proof)
    DropWorkerOfRequest {
        request_id: B256,
    },
}

impl fmt::Debug for WorkerRegistryCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let command = match self {
            WorkerRegistryCommand::AssignProofRequest { .. } => "AssignProofRequest",
            WorkerRegistryCommand::WorkerReady { .. } => "WorkerReady",
            WorkerRegistryCommand::ProofComplete { .. } => "ProofComplete",
            WorkerRegistryCommand::ProofProgressUpdate { .. } => "ProofProgressUpdate",
            WorkerRegistryCommand::ProofStatusRequest { .. } => "ProofStatusRequest",
            WorkerRegistryCommand::ProofStatusResponse { .. } => "ProofStatusResponse",
            WorkerRegistryCommand::Workers { .. } => "Workers",
            WorkerRegistryCommand::DeadWorkers { .. } => "DeadWorkers",
            WorkerRegistryCommand::ProofHistory { .. } => "ProofHistory",
            WorkerRegistryCommand::Heartbeat { .. } => "Heartbeat",
            WorkerRegistryCommand::StrikeWorkerOfRequest { .. } => "StrikeWorkerOfRequest",
            WorkerRegistryCommand::DropWorkerOfRequest { .. } => "DropWorkerOfRequest",
        };
        write!(f, "{command}")
    }
}
