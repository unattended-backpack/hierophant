use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::{
    SP1Proof, SP1ProofWithPublicValues, SP1PublicValues, SP1Stdin,
    network::proto::network::ProofMode,
};
use std::{cmp::Ordering, fmt::Display};

pub const REGISTER_CONTEMPLANT_ENDPOINT: &str = "register_contemplant";
// Increment this whenever there is a breaking change in the contemplant
// This is to ensure the contemplant is on the same version as the Hierophant it's
// connecting to
pub const CONTEMPLANT_VERSION: &str = "5.0.0";

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

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
pub struct ContemplantProofStatus {
    pub execution_status: i32,
    pub proof: Option<Vec<u8>>,
    pub progress: Option<ProgressUpdate>,
}

impl ContemplantProofStatus {
    pub fn unexecuted() -> Self {
        Self {
            execution_status: ExecutionStatus::Unexecuted.into(),
            proof: None,
            progress: None,
        }
    }

    // Progress can never go from Some(progress) to None.  Will always take the higher progress
    pub fn progress_update(&mut self, new: Option<ProgressUpdate>) {
        let updated_progress = match (self.progress, new) {
            (Some(progress), Some(new_progress)) => Some(progress.max(new_progress)),
            (Some(progress), None) => Some(progress),
            (None, Some(progress)) => Some(progress),
            (None, None) => None,
        };
        self.progress = updated_progress;
    }

    pub fn default() -> Self {
        Self {
            execution_status: ExecutionStatus::Unexecutable.into(),
            proof: None,
            progress: None,
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

        let progress_update = match self.progress {
            Some(ProgressUpdate::Execution(x)) => format!("{x}% executed"),
            Some(ProgressUpdate::Serialization(x)) => format!("{x}% serialized"),
            Some(ProgressUpdate::Done) => format!("Done"),
            None => format!("not started"),
        };

        write!(
            f,
            "ExecutionStatus: {}, Progress: {}, Proof: {}",
            execution_status.as_str_name(),
            progress_update,
            proof
        )
    }
}

// contemplant's progress on their current proof
#[derive(Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
pub enum ProgressUpdate {
    Execution(u64),     // 0 to 100
    Serialization(u64), // 0 to 100
    Done,               // Finished with a count of serialization shards.
}

impl Display for ProgressUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ProgressUpdate::Execution(x) => {
                format!("{x}% executed")
            }
            ProgressUpdate::Serialization(x) => {
                format!("{x}% serialized")
            }
            ProgressUpdate::Done => {
                format!("done")
            }
        };

        write!(f, "{}", msg)
    }
}

impl Default for ProgressUpdate {
    fn default() -> Self {
        Self::Execution(0)
    }
}

impl PartialOrd for ProgressUpdate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProgressUpdate {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            // Done is always greatest
            (ProgressUpdate::Done, ProgressUpdate::Done) => Ordering::Equal,
            (ProgressUpdate::Done, _) => Ordering::Greater,
            (_, ProgressUpdate::Done) => Ordering::Less,

            // Serialization > Execution
            (ProgressUpdate::Serialization(_), ProgressUpdate::Execution(_)) => Ordering::Greater,
            (ProgressUpdate::Execution(_), ProgressUpdate::Serialization(_)) => Ordering::Less,

            // Same variant - compare by value
            (ProgressUpdate::Execution(x), ProgressUpdate::Execution(y)) => x.cmp(y),
            (ProgressUpdate::Serialization(x), ProgressUpdate::Serialization(y)) => x.cmp(y),
        }
    }
}

/// A proof generated by the SP1 RISC-V zkVM bundled together with the public values and the
/// version.
/*
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SP1ProofWithPublicValues {
    /// The raw proof generated by the SP1 RISC-V zkVM.
    pub proof: SP1Proof,
    /// The public values generated by the SP1 RISC-V zkVM.
    pub public_values: SP1PublicValues,
    /// The version of the SP1 RISC-V zkVM (not necessary but useful for detecting version
    /// mismatches).
    pub sp1_version: String,
    /// The integrity proof generated by the TEE server.
    pub tee_proof: Option<Vec<u8>>,
}
*/
// The proof generated by the prover network.
//
// Since [`bincode`] is not self describing, it cannot handle "nullable" optional values.
//
// This is the same exact type as ProofFromNetwork in sp1_sdk but their's isn't public
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofFromNetwork {
    proof: SP1Proof,
    public_values: SP1PublicValues,
    sp1_version: String,
}

impl From<ProofFromNetwork> for SP1ProofWithPublicValues {
    fn from(value: ProofFromNetwork) -> Self {
        Self {
            proof: value.proof,
            public_values: value.public_values,
            sp1_version: value.sp1_version,
            tee_proof: None,
        }
    }
}

impl From<SP1ProofWithPublicValues> for ProofFromNetwork {
    fn from(value: SP1ProofWithPublicValues) -> Self {
        Self {
            proof: value.proof,
            public_values: value.public_values,
            sp1_version: value.sp1_version,
        }
    }
}
