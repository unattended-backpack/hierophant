use crate::{ContemplantProofRequest, ContemplantProofStatus, WorkerRegisterInfo};
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Serialize, Deserialize)]
pub enum FromContemplantMessage {
    // sent from contemplant to hierophant on startup
    Register(WorkerRegisterInfo),
    // sends proof_status responses to the hierophant
    ProofStatusResponse(B256, Option<ContemplantProofStatus>),
    Heartbeat,
}

impl Display for FromContemplantMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::Register(_) => "Register",
            Self::ProofStatusResponse(_, _) => "ProofStatusResponse",
            Self::Heartbeat => "Heartbeat",
        };
        write!(f, "{msg}")
    }
}

#[derive(Serialize, Deserialize)]
pub enum FromHierophantMessage {
    // sent from hierophant to contemplant to start working on a new proof
    ProofRequest(ContemplantProofRequest),
    // sent from hierophant to contemplant to get the status of a proof
    ProofStatusRequest(B256),
}

impl Display for FromHierophantMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            Self::ProofRequest(_) => "ProofRequest",
            Self::ProofStatusRequest(_) => "ProofStatusRequest",
        };
        write!(f, "{msg}")
    }
}
