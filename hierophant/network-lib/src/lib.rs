use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::{SP1Stdin, network::proto::network::ProofMode};
use std::fmt::Display;

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub ip: String,
    pub port: usize,
}

/*
// Is deterministic on a RequestProofRequestBody using the fields
// vk_hash, version, mode, and stdin_uri.  This way we can
// skip execution for proofs we already have saved
// TODO: I think this isn't being used properly rn because we removed proof_cache and proofs aren't
// stored by their ProofRequestId in
#[derive(Default, Debug, Clone, Copy, Serialize, Eq, PartialEq, Hash, Deserialize)]
pub struct ProofRequestId(B256);

impl ProofRequestId {
    pub fn new(vk_hash: Vec<u8>, version: String, stdin_uri: String, mode: i32) -> Self {
        let mut hasher = Sha256::new();

        // Hash the minimum fields that make proof execution distinct
        // TODO: is this really the minimum fields
        hasher.update(vk_hash.clone());
        hasher.update(version.clone());
        hasher.update(stdin_uri.clone());
        hasher.update(mode.to_le_bytes());

        // turn it into B256 (how sp1_sdk represents proof_id)
        let hash = hasher.finalize().to_vec();

        Self(B256::from_slice(&hash))
    }
}

impl From<ProofRequestId> for Vec<u8> {
    fn from(id: ProofRequestId) -> Vec<u8> {
        id.0.to_vec()
    }
}

impl From<B256> for ProofRequestId {
    fn from(b256: B256) -> ProofRequestId {
        ProofRequestId(b256)
    }
}

impl TryFrom<Vec<u8>> for ProofRequestId {
    type Error = &'static str;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(ProofRequestId(B256::from_slice(&bytes)))
    }
}

impl Display for ProofRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
*/

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

        write!(
            f,
            "ProofRequest request_id: {request_id}, mock: {mock}, mode: {mode}",
        )
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
