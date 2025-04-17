use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Display;

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub port: usize,
}

// Is deterministic on a RequestProofRequestBody using the fields
// vk_hash, version, mode, strategy, and stdin_uri.  This way we can
// skip execution for proofs we already have saved
#[derive(Debug, Clone, Copy, Serialize, Eq, PartialEq, Hash, Deserialize)]
pub struct ProofRequestId(B256);

impl ProofRequestId {
    pub fn new(vk_hash: Vec<u8>, version: String, stdin_uri: String) -> Self {
        let mut hasher = Sha256::new();

        // Hash the minimum fields that make proof execution distinct
        // TODO: is this really the minimum fields
        hasher.update(vk_hash.clone());
        hasher.update(version.clone());
        hasher.update(stdin_uri.clone());

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
