use log::debug;
use network_lib::ProofRequestId;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt::{self, Display},
    net::SocketAddr,
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum WorkerStatus {
    Idle,
    Busy { request_id: ProofRequestId },
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy { request_id } => write!(f, "Busy with proof request {request_id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkerState {
    pub name: String,
    pub id: Uuid,
    pub status: WorkerStatus,
    // publicly callable address
    pub address: SocketAddr,
    pub strikes: usize,
}

impl WorkerState {
    pub fn new(name: String, address: SocketAddr) -> WorkerState {
        let id = Uuid::new_v4();
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
    fn assign_proof(&mut self, request_id: ProofRequestId) {
        self.status = WorkerStatus::Busy { request_id };
        // This worker has been good.  Reset their strikes
        self.strikes = 0;
    }

    fn should_drop(&self, cfg_max_worker_strikes: usize) -> bool {
        self.strikes >= cfg_max_worker_strikes
    }

    // returns the proof the worker is currently working on, if any
    fn current_proof_id(&self) -> Option<ProofRequestId> {
        match &self.status {
            WorkerStatus::Idle => None,
            WorkerStatus::Busy { request_id } => Some(request_id.clone()),
        }
    }
}
