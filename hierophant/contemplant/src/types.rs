use alloy_primitives::B256;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sp1_sdk::{
    SP1Stdin,
    network::proto::network::{ExecutionStatus, ProofMode, RequestProofRequestBody},
};
use std::{collections::HashMap, fmt::Display};
use tokio::sync::RwLock;

pub type ProofStore = RwLock<HashMap<B256, ProofStatus>>;

// TODO: (maybe) Gas limit and cycle limit
#[derive(Serialize, Deserialize)]
pub struct ProofRequest {
    pub proof_id: B256,
    pub elf: Vec<u8>,
    pub mode: ProofMode,
    pub sp1_stdin: SP1Stdin,
}

impl Display for ProofRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let proof_id = self.proof_id;
        let mode = self.mode.as_str_name();

        write!(f, "proof_id: {proof_id}, mode: {mode}",)
    }
}

/*
pub enum ExecutionStatus {
    UnspecifiedExecutionStatus = 0,
    /// The request has not been executed.
    Unexecuted = 1,
    /// The request has been executed.
    Executed = 2,
    /// The request cannot be executed.
    Unexecutable = 3,
}
*/

#[derive(Serialize, Deserialize, Clone)]
pub struct ProofStatus {
    execution_status: i32,
    proof: Option<Vec<u8>>,
}

impl ProofStatus {
    pub fn unspecified() -> Self {
        Self {
            execution_status: ExecutionStatus::UnspecifiedExecutionStatus.into(),
            proof: None,
        }
    }

    pub fn unexecuted() -> Self {
        Self {
            execution_status: ExecutionStatus::UnspecifiedExecutionStatus.into(),
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

impl Display for ProofStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let execution_status = match self.execution_status {
            0 => "UnspecifiedExecutionStatus",
            1 => "Unexecuted",
            2 => "Executed",
            3 => "Unexecutable",
            _ => "Display Error: Unknown execution execution status",
        };

        let proof = match self.proof {
            Some(_) => "some",
            None => "none",
        };

        write!(f, "ExecutionStatus: {}, Proof: {}", execution_status, proof)
    }
}

pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", self.0)).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
