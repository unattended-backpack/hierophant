pub mod messages;
pub mod protocol;

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};
use sp1_sdk::ProofFromNetwork;
// The sp1-sdk 6.x release split the old `proto::network` module into
// `proto::base` and `proto::auction`. Hierophant serves the base
// (Reserved/hosted) flow, the one a `NetworkMode::Reserved` client drives
// end-to-end, so these shared types mirror `base::types`. The enum wire values are identical in both
// modules, so contemplants and hierophant agree regardless.
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use sp1_sdk::{SP1ProofWithPublicValues, SP1Stdin, network::proto::base::types::ProofMode};
use std::{cmp::Ordering, fmt::Display};

// Which ZK VM a proof request targets, and which VMs a given contemplant is
// configured to serve.  Registry uses this to filter idle workers when routing.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum VmKind {
    Sp1,
    Risc0,
    OpenVm,
}

impl VmKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sp1 => "SP1",
            Self::Risc0 => "RISC0",
            Self::OpenVm => "OPENVM",
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
    // Whether this contemplant can produce OpenVM EVM (halo2-wrapped) proofs.
    // Opt-in because the halo2 path needs the KZG params installed by
    // `cargo openvm setup` (tens of GB) plus a very heavy aggregation keygen;
    // a worker without those assets leaves this false so hierophant won't
    // route EVM-mode OpenVM work to it. Meaningful only when supported_vms
    // contains VmKind::OpenVm.
    #[serde(default)]
    pub openvm_evm_enabled: bool,
    // endpoint to hit to drop this contemplant from it's Magister.
    // Only Some if this contemplant has a Magister
    pub magister_drop_endpoint: Option<String>,
    // Random per-process nonce, fresh at contemplant startup. Lets the
    // hierophant distinguish a ws reconnect from the SAME process (its
    // in-flight assignment is still being proven — carry it over) from
    // a restarted process (all in-memory proof state lost — register
    // fresh).
    pub instance_nonce: u64,
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
        let openvm_evm = if self.openvm_evm_enabled {
            ", openvm-evm"
        } else {
            ""
        };
        write!(
            f,
            "{} CONTEMPLANT_VERSION {} [VMs: {}{}{}]{}",
            self.name, self.contemplant_version, vms, groth16, openvm_evm, magister_info
        )
    }
}

// VM-tagged proof request.  The enum discriminant is the routing key used by
// the worker registry to match requests against workers' supported_vms.
#[derive(Clone, Serialize, Deserialize)]
pub enum ContemplantProofRequest {
    Sp1(Sp1ProofRequest),
    Risc0(Risc0ProofRequest),
    OpenVm(OpenVmProofRequest),
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

#[derive(Clone, Serialize, Deserialize)]
pub struct OpenVmProofRequest {
    pub request_id: B256,
    // Raw RISC-V ELF as produced by `cargo openvm build --no-transpile`.
    // The contemplant's OpenVmExecutor decodes + transpiles it against the
    // request's VM config; hierophant does the same when verifying the
    // returned proof, so both sides agree on the program commitment.
    pub elf: Vec<u8>,
    // Optional `openvm.toml` app-config contents (the `[app_vm_config.*]`
    // tables that declare which VM extensions the guest was built with).
    // None means the SDK's standard rv32im + io config. Hierophant treats
    // this opaquely; both the contemplant (proving) and the hierophant
    // (verifying) parse it with openvm-sdk so keygen is identical on both
    // sides.
    pub app_config_toml: Option<String>,
    // Input streams for the guest's StdIn, in write order. Each entry is one
    // hint stream, written via `StdIn::write_bytes`; the guest consumes them
    // in order with `read_vec()` (or `read()` when the bytes are an
    // openvm-serde encoding). Opaque to hierophant.
    pub input: Vec<Vec<u8>>,
    pub mode: OpenVmProofMode,
    pub mock: bool,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub enum OpenVmProofMode {
    // App-level STARK (continuation proof); the default and cheapest mode.
    App,
    // Aggregated root STARK; single compact proof over the whole execution.
    Stark,
    // Halo2-wrapped, EVM-verifiable SNARK. Requires a worker that registered
    // with openvm_evm_enabled=true (KZG params + aggregation keys present).
    Evm,
}

impl OpenVmProofMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::App => "APP",
            Self::Stark => "STARK",
            Self::Evm => "EVM",
        }
    }
}

impl ContemplantProofRequest {
    pub fn request_id(&self) -> B256 {
        match self {
            Self::Sp1(r) => r.request_id,
            Self::Risc0(r) => r.request_id,
            Self::OpenVm(r) => r.request_id,
        }
    }

    pub fn vm(&self) -> VmKind {
        match self {
            Self::Sp1(_) => VmKind::Sp1,
            Self::Risc0(_) => VmKind::Risc0,
            Self::OpenVm(_) => VmKind::OpenVm,
        }
    }

    pub fn is_mock(&self) -> bool {
        match self {
            Self::Sp1(r) => r.mock,
            Self::Risc0(r) => r.mock,
            Self::OpenVm(r) => r.mock,
        }
    }

    pub fn mode_name(&self) -> String {
        match self {
            Self::Sp1(r) => r.mode.as_str_name().to_string(),
            Self::Risc0(r) => r.mode.as_str().to_string(),
            Self::OpenVm(r) => r.mode.as_str().to_string(),
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
            Self::OpenVm(_) => false,
        }
    }

    // Returns true when serving this request requires the worker to have the
    // OpenVM halo2/EVM toolchain available (KZG params from
    // `cargo openvm setup` + aggregation keys). Used by the worker registry
    // to skip workers that registered with openvm_evm_enabled=false.
    pub fn needs_openvm_evm(&self) -> bool {
        match self {
            Self::Sp1(_) | Self::Risc0(_) => false,
            Self::OpenVm(r) => r.mode == OpenVmProofMode::Evm,
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
            Some(p) => p.to_string(),
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

// The phase of a proof a contemplant is currently in. Ordered by the order
// they run in, so progress is monotonic across a phase transition even
// though each phase's `done` counter restarts from zero. Every zkVM's
// proving pipeline maps onto these four:
//   Execute   - running the guest / counting shards|segments (cheap)
//   Prove     - per-shard|per-segment STARK proving (the expensive core)
//   Aggregate - recursion / compression of the STARK proof tree
//   Wrap      - SNARK wrap (PLONK/Groth16/halo2) for on-chain verification
#[derive(Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
pub enum ProvePhase {
    Execute,
    Prove,
    Aggregate,
    Wrap,
}

impl ProvePhase {
    // Monotonic rank used by the Ord impl below.
    pub fn rank(&self) -> u8 {
        match self {
            ProvePhase::Execute => 0,
            ProvePhase::Prove => 1,
            ProvePhase::Aggregate => 2,
            ProvePhase::Wrap => 3,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProvePhase::Execute => "execute",
            ProvePhase::Prove => "prove",
            ProvePhase::Aggregate => "aggregate",
            ProvePhase::Wrap => "wrap",
        }
    }
}

// contemplant's progress on their current proof. VM-neutral: each executor
// reports which phase it is in and how much of that phase is done. `total`
// == 0 means indeterminate - a live per-unit tick with no reachable total
// (SP1's opaque gpu-server, or any coarse phase); `done` still advances, so
// it remains a real liveness signal even without a percentage.
#[derive(Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Clone)]
pub enum ProgressUpdate {
    Phase { phase: ProvePhase, done: u64, total: u64 },
    Done, // Finished
}

impl ProgressUpdate {
    // Convenience constructors for the executors.
    pub fn phase(phase: ProvePhase, done: u64, total: u64) -> Self {
        ProgressUpdate::Phase { phase, done, total }
    }
    pub fn indeterminate(phase: ProvePhase, done: u64) -> Self {
        ProgressUpdate::Phase { phase, done, total: 0 }
    }
}

impl Display for ProgressUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgressUpdate::Phase { phase, done, total } => {
                if *total > 0 {
                    let pct = (*done).saturating_mul(100) / (*total);
                    write!(f, "{}: {done}/{total} ({pct}%)", phase.as_str())
                } else {
                    write!(f, "{}: {done}", phase.as_str())
                }
            }
            ProgressUpdate::Done => write!(f, "done"),
        }
    }
}

impl Default for ProgressUpdate {
    fn default() -> Self {
        ProgressUpdate::Phase {
            phase: ProvePhase::Execute,
            done: 0,
            total: 0,
        }
    }
}

impl PartialOrd for ProgressUpdate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Progress order: Done is greatest; otherwise by (phase rank, work done).
// Phase rank dominates so a phase transition always counts as progress even
// though `done` resets, and within a phase a rising `done` (shard/segment
// count or percent) counts as progress. `total` is display-only and is
// intentionally NOT part of the ordering.
impl Ord for ProgressUpdate {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (ProgressUpdate::Done, ProgressUpdate::Done) => Ordering::Equal,
            (ProgressUpdate::Done, _) => Ordering::Greater,
            (_, ProgressUpdate::Done) => Ordering::Less,
            (
                ProgressUpdate::Phase { phase: pa, done: da, .. },
                ProgressUpdate::Phase { phase: pb, done: db, .. },
            ) => pa.rank().cmp(&pb.rank()).then_with(|| da.cmp(db)),
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
