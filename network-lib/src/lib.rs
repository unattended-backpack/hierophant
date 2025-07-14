pub mod messages;
pub mod protocol;

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sp1_sdk::ProofFromNetwork;
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::{SP1ProofWithPublicValues, SP1Stdin, network::proto::network::ProofMode};
use std::{cmp::Ordering, fmt::Display};

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub contemplant_version: String,
    // endpoint to hit to drop this contemplant from it's Magister.
    // Only Some if this contemplant has a Magister
    pub magister_drop_endpoint: Option<String>,
}

impl Display for WorkerRegisterInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let magister_info = match self.magister_drop_endpoint.clone() {
            Some(x) => format!(" with Magister drop endpoint {x}"),
            None => "".to_string(),
        };
        write!(
            f,
            "{} CONTEMPLANT_VERSION {}{}",
            self.name, self.contemplant_version, magister_info
        )
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
}

impl Default for ContemplantProofStatus {
    fn default() -> Self {
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
            Some(ProgressUpdate::Done) => "Done".to_string(),
            None => "not started".to_string(),
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
    Done,               // Finished
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
            ProgressUpdate::Done => "done".to_string(),
        };

        write!(f, "{msg}")
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

// Progress order goes Execution(0) -> Execution(100) -> Serialization(0) -> Serialization(100) -> Done
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

// helper function
// sp1_sdk doesn't impl From<SP1ProofWithPublicValues> for ProofFromNetwork so we have to make a
// fake impl.
pub fn to_proof_from_network(p: SP1ProofWithPublicValues) -> ProofFromNetwork {
    ProofFromNetwork {
        proof: p.proof,
        public_values: p.public_values,
        sp1_version: p.sp1_version,
    }
}
