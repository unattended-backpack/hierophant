pub mod messages;
pub mod protocol;

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sp1_sdk::ProofFromNetwork;
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::{SP1ProofWithPublicValues, SP1Stdin, network::proto::network::ProofMode};
use std::{cmp::Ordering, fmt::Display};

// Which ZK VM a proof request targets, and which VMs a given contemplant is
// configured to serve.  Registry uses this to filter idle workers when routing.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum VmKind {
    Sp1,
    Risc0,
}

impl VmKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sp1 => "SP1",
            Self::Risc0 => "RISC0",
        }
    }
}

impl Display for VmKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerRegisterInfo {
    pub name: String,
    pub contemplant_version: String,
    pub supported_vms: Vec<VmKind>,
    // Whether this contemplant can produce RISC Zero Groth16 proofs (fresh
    // or as a STARK → Groth16 wrap). Opt-in because the groth16 path needs
    // the vendored prover assets under /opt/risc0-groth16-prover/ and the
    // docker shim the contemplant image installs; a worker without those
    // assets leaves this false so hierophant won't route Groth16 work to it.
    // Meaningful only when supported_vms contains VmKind::Risc0.
    #[serde(default)]
    pub groth16_enabled: bool,
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
        let vms = self
            .supported_vms
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let groth16 = if self.groth16_enabled { ", groth16" } else { "" };
        write!(
            f,
            "{} CONTEMPLANT_VERSION {} [VMs: {}{}]{}",
            self.name, self.contemplant_version, vms, groth16, magister_info
        )
    }
}

// VM-tagged proof request.  The enum discriminant is the routing key used by
// the worker registry to match requests against workers' supported_vms.
#[derive(Clone, Serialize, Deserialize)]
pub enum ContemplantProofRequest {
    Sp1(Sp1ProofRequest),
    Risc0(Risc0ProofRequest),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Sp1ProofRequest {
    pub request_id: B256,
    pub elf: Vec<u8>,
    pub mock: bool,
    pub mode: ProofMode,
    pub sp1_stdin: SP1Stdin,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Risc0ProofRequest {
    pub request_id: B256,
    pub elf: Vec<u8>,
    // Raw input bytes to be written into the guest's ExecutorEnv.  Hierophant
    // treats this opaquely; contemplant's Risc0Executor writes it via
    // ExecutorEnvBuilder::write_slice.
    pub input: Vec<u8>,
    pub mode: Risc0ProofMode,
    pub mock: bool,
    // If set, this is a two-step STARK → Groth16 wrap, not a fresh proof.
    // The contemplant's Risc0Executor deserializes these bytes as a prior
    // Receipt and runs `prover.compress(&receipt, &ProverOpts::groth16())`
    // instead of `prove_with_opts(elf, input, ...)`. When present, `elf` and
    // `input` are ignored (but must be valid bincode-wise because they're
    // part of the struct; typically passed as empty Vecs).
    //
    // This backs the Bonsai `POST /snark/create` flow: a client finishes a
    // STARK session, then asks us to wrap its receipt into a Groth16 seal
    // suitable for onchain verification.
    #[serde(default)]
    pub wrap_of: Option<Vec<u8>>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum Risc0ProofMode {
    Composite,
    Succinct,
    Groth16,
}

impl Risc0ProofMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Composite => "COMPOSITE",
            Self::Succinct => "SUCCINCT",
            Self::Groth16 => "GROTH16",
        }
    }
}

impl ContemplantProofRequest {
    pub fn request_id(&self) -> B256 {
        match self {
            Self::Sp1(r) => r.request_id,
            Self::Risc0(r) => r.request_id,
        }
    }

    pub fn vm(&self) -> VmKind {
        match self {
            Self::Sp1(_) => VmKind::Sp1,
            Self::Risc0(_) => VmKind::Risc0,
        }
    }

    pub fn is_mock(&self) -> bool {
        match self {
            Self::Sp1(r) => r.mock,
            Self::Risc0(r) => r.mock,
        }
    }

    pub fn mode_name(&self) -> String {
        match self {
            Self::Sp1(r) => r.mode.as_str_name().to_string(),
            Self::Risc0(r) => r.mode.as_str().to_string(),
        }
    }

    // Returns true when serving this request requires the worker to have the
    // RISC Zero Groth16 toolchain available (vendored assets + docker shim).
    // Covers both fresh Groth16 proofs and STARK → Groth16 wrap jobs (which
    // always target Groth16). Used by the worker registry to skip workers
    // that registered with groth16_enabled=false.
    pub fn needs_groth16(&self) -> bool {
        match self {
            Self::Sp1(_) => false,
            Self::Risc0(r) => r.mode == Risc0ProofMode::Groth16 || r.wrap_of.is_some(),
        }
    }
}

impl Display for ContemplantProofRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mock = if self.is_mock() { "mock " } else { "" };
        write!(
            f,
            "{vm} {mock}{mode} proof with request id {id}",
            vm = self.vm(),
            mode = self.mode_name(),
            id = self.request_id()
        )
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
