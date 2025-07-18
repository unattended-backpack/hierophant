use crate::network::{ExecutionStatus, FulfillmentStatus};
use network_lib::ContemplantProofStatus;
use serde::{Deserialize, Serialize};
use std::fmt::Display;

/*
pub enum FulfillmentStatus {
    UnspecifiedFulfillmentStatus = 0,
    /// Proof request is pending
    Pending = 1,
    /// Proof request is assigned to a prover
    Assigned = 2,
    /// Proof has been generated
    Fulfilled = 3,
    /// Proof generation failed
    Failed = 4,
    /// Proof request was cancelled
    Cancelled = 5,
}

pub enum ExecutionStatus {
    UnspecifiedExecutionStatus = 0,
    /// Execution is pending
    Unexecuted = 1,
    /// Execution completed successfully
    Executed = 2,
    /// Execution failed
    Unexecutable = 3,
}
*/

#[derive(Serialize, Deserialize, Debug)]
/// The status of a proof request.
pub struct ProofStatus {
    // Note: Can't use `FulfillmentStatus`/`ExecutionStatus` directly because `Serialize_repr` and `Deserialize_repr` aren't derived on it.
    pub fulfillment_status: i32,
    pub execution_status: i32,
    pub proof: Vec<u8>,
}

// ContemplantProofStatus structs are only constructed using it's methods `unexected()`,
// `executed` or `unexectable` so it will only have a proof when it's executed.  Otherwise
// this would be unsound code.  The invariant is that an ExecutionStatus::Executed will always be
// accompanied by a proof
impl From<ContemplantProofStatus> for ProofStatus {
    fn from(contemplant_proof_status: ContemplantProofStatus) -> Self {
        match ExecutionStatus::try_from(contemplant_proof_status.execution_status)
            .unwrap_or(ExecutionStatus::UnspecifiedExecutionStatus)
        {
            ExecutionStatus::UnspecifiedExecutionStatus => Self {
                fulfillment_status: FulfillmentStatus::UnspecifiedFulfillmentStatus.into(),
                execution_status: ExecutionStatus::UnspecifiedExecutionStatus.into(),
                proof: vec![],
            },
            ExecutionStatus::Unexecuted => Self {
                fulfillment_status: FulfillmentStatus::Assigned.into(),
                execution_status: ExecutionStatus::Unexecuted.into(),
                proof: vec![],
            },
            ExecutionStatus::Executed => Self {
                fulfillment_status: FulfillmentStatus::Fulfilled.into(),
                execution_status: ExecutionStatus::Executed.into(),
                // we could default to an empty vector here, but if this is ever the case then
                // there is a larger bug within the codebase, and returning an empty vector can be
                // considered returing an incorrect proof. Additionally, we definitely want this to
                // fail loudly so we can be notified and debug where our invariant is violated.
                proof: contemplant_proof_status.proof.unwrap(),
            },
            ExecutionStatus::Unexecutable => Self {
                fulfillment_status: FulfillmentStatus::Unfulfillable.into(),
                execution_status: ExecutionStatus::Unexecutable.into(),
                proof: vec![],
            },
        }
    }
}

impl ProofStatus {
    /*
    If the fulfillment_status is Unfulfillable && ExecutionStatus is NOT Unexecutable then the proposer will re-try the same span proof request.
    We set the fulfillment_status to Unfulfillable because we want the proposer to re-try this request but we set execution status to Unspecified because
    we DON'T want the proposer to split it into 2 requests

    [from op-succinct/validity/src/proof_requester ln 303]:
    If the request is a range proof and the number of failed requests is greater than 2 or the execution status is unexecutable, the request is split into two new requests.
    Otherwise, add_new_ranges will insert the new request. This ensures better failure-resilience. If the request to add two range requests fails, add_new_ranges will handle it gracefully by submitting
    the same range.
    */
    pub fn lost() -> Self {
        Self {
            fulfillment_status: FulfillmentStatus::Unfulfillable.into(),
            execution_status: ExecutionStatus::UnspecifiedExecutionStatus.into(),
            proof: vec![],
        }
    }

    pub fn assigned() -> Self {
        Self {
            fulfillment_status: FulfillmentStatus::Assigned.into(),
            execution_status: ExecutionStatus::Unexecuted.into(),
            proof: vec![],
        }
    }
}

impl Display for ProofStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let fulfillment_status = FulfillmentStatus::try_from(self.fulfillment_status)
            .unwrap_or(FulfillmentStatus::UnspecifiedFulfillmentStatus)
            .as_str_name();

        let execution_status = ExecutionStatus::try_from(self.execution_status)
            .unwrap_or(ExecutionStatus::UnspecifiedExecutionStatus)
            .as_str_name();

        let proof_display = if self.proof.is_empty() {
            "Empty"
        } else {
            "Non-empty"
        };

        write!(
            f,
            "FulfillmentStatus: {fulfillment_status}, ExecutionStatus: {execution_status}, Proof: {proof_display}"
        )
    }
}
