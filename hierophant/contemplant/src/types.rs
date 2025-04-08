use serde::{Deserialize, Serialize};
use sp1_sdk::network::proto::network::ExecutionStatus;

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

#[derive(Serialize, Deserialize)]
pub struct ProofStatus {
    execution_status: i32,
    proof: Option<Vec<u8>>,
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

        write!(
            f,
            "ExecutionStatus: {}, Proof: {}",
            execution_status, proof_display
        )
    }
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

    pub fn executed(proof: Vec<u8>) -> Self {
        Self {
            execution_status: ExecutionStatus::Executed.into(),
            proof: Some(proof),
        }
    }

    pub fn unexecutable() -> Self {
        Self {
            execution_status: ExecutionStatus::Unexecutable.into(),
            proof: None,
        }
    }
}
