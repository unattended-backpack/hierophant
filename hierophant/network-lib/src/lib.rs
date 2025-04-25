use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::{SP1Stdin, network::proto::network::ProofMode};
use std::fmt::Display;

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";
// Increment this whenever there is a breaking change in the contemplant
// This is to ensure the contemplant is on the same version as the Hierophant it's
// connecting to
pub const CONTEMPLANT_VERSION: &str = "2.0.0";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub contemplant_version: String,
}

impl Display for WorkerRegisterInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} CONTEMPLANT_VERSION {}",
            self.name, self.contemplant_version
        )
    }
}

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
        write!(f, "FromContemplantMessage display TODO")
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
        write!(f, "FromHierophantMessage display TODO")
    }
}

// TODO: (maybe) Gas limit and cycle limit
#[derive(Serialize, Deserialize)]
pub struct ContemplantProofRequest {
    pub request_id: B256,
    pub elf: Vec<u8>,
    pub mock: bool,
    pub mode: ProofMode,
    pub sp1_stdin: SP1Stdin,
}

impl Display for ContemplantProofRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request_id = self.request_id;
        let mock = self.mock;
        let mode = self.mode.as_str_name();

        if mock {
            write!(f, "{mode} mock proof with request id {request_id}")
        } else {
            write!(f, "{mode} proof with request id {request_id}")
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ContemplantProofStatus {
    pub execution_status: i32,
    pub proof: Option<Vec<u8>>,
}

impl ContemplantProofStatus {
    pub fn unexecuted() -> Self {
        Self {
            execution_status: ExecutionStatus::Unexecuted.into(),
            proof: None,
        }
    }

    pub fn executed(proof_bytes: Vec<u8>) -> Self {
        Self {
            execution_status: ExecutionStatus::Executed.into(),
            proof: Some(proof_bytes),
        }
    }

    pub fn unexecutable() -> Self {
        Self {
            execution_status: ExecutionStatus::Unexecutable.into(),
            proof: None,
        }
    }
}

impl Display for ContemplantProofStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let execution_status = ExecutionStatus::try_from(self.execution_status)
            .unwrap_or(ExecutionStatus::UnspecifiedExecutionStatus);

        let proof = match self.proof {
            Some(_) => "some",
            None => "none",
        };

        write!(
            f,
            "ExecutionStatus: {}, Proof: {}",
            execution_status.as_str_name(),
            proof
        )
    }
}
