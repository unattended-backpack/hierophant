use crate::config::WorkerRegistryConfig;
use alloy_primitives::B256;
use anyhow::Result;
use log::{debug, warn};
use network_lib::{ProgressUpdate, VmKind, messages::FromHierophantMessage};
use serde::{Serialize, Serializer};
use std::fmt::Display;
use std::time::SystemTime;
use tokio::{sync::mpsc, time::Instant};

#[derive(Clone, Debug, Serialize)]
pub struct WorkerState {
    pub name: String,
    pub status: WorkerStatus,
    // VM kinds + per-VM sub-capabilities, consolidated (replaces the old
    // supported_vms + groth16_enabled + openvm_evm_enabled trio).
    pub capabilities: Capabilities,
    pub strikes: usize,
    // Whole-seconds age of the last heartbeat from this contemplant — the
    // liveness signal ("is this worker alive / has it gone quiet?"). In the
    // cycle-rate ETA model the ETA is a static estimate, so the heartbeat
    // (not a progress tick) is what tells an observer the worker is alive.
    #[serde(rename = "last_heartbeat_secs", serialize_with = "serialize_instant_age_secs")]
    pub last_heartbeat: Instant,
    #[serde(skip_serializing)]
    pub from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    // Control-plane URL for dropping this contemplant from its Magister;
    // internal, not part of the observability view.
    #[serde(skip_serializing)]
    pub magister_drop_endpoint: Option<String>,
    // Per-startup nonce for reconnect-vs-restart detection; internal.
    #[serde(skip_serializing)]
    pub instance_nonce: u64,
}

// A contemplant's proving capabilities, as reported at registration.
#[derive(Clone, Debug, Serialize)]
pub struct Capabilities {
    pub vms: Vec<VmKind>,
    // RISC Zero Groth16 (fresh or STARK -> Groth16 wrap). Meaningful only
    // when `vms` contains Risc0.
    pub risc0_groth16: bool,
    // OpenVM EVM (halo2-wrapped). Meaningful only when `vms` contains OpenVm.
    pub openvm_evm: bool,
}

impl WorkerState {
    pub(super) fn new(
        name: String,
        supported_vms: Vec<VmKind>,
        groth16_enabled: bool,
        openvm_evm_enabled: bool,
        magister_drop_endpoint: Option<String>,
        instance_nonce: u64,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) -> Self {
        Self {
            name,
            status: WorkerStatus::Idle,
            capabilities: Capabilities {
                vms: supported_vms,
                risc0_groth16: groth16_enabled,
                openvm_evm: openvm_evm_enabled,
            },
            strikes: 0,
            last_heartbeat: Instant::now(),
            from_hierophant_sender,
            magister_drop_endpoint,
            instance_nonce,
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
    }

    pub(super) fn supports(&self, vm: VmKind) -> bool {
        self.capabilities.vms.contains(&vm)
    }

    // True iff this worker can serve the given request. Combines the VM-kind
    // filter with the per-VM capability filters (RISC Zero Groth16, OpenVM
    // EVM) so `handle_assign_proof` has a single predicate to check.
    pub(super) fn can_serve(&self, request: &network_lib::ContemplantProofRequest) -> bool {
        if !self.supports(request.vm()) {
            return false;
        }
        if request.needs_groth16() && !self.capabilities.risc0_groth16 {
            return false;
        }
        if request.needs_openvm_evm() && !self.capabilities.openvm_evm {
            return false;
        }
        true
    }

    // Return the worker to Idle after a successful proof. Per-(vm, mode)
    // throughput is now learned contemplant-side (see rate_model), so the
    // hierophant no longer tracks proof-time averages here.
    pub(super) fn completed_proof(&mut self) {
        self.status = WorkerStatus::Idle;
        self.strikes = 0;
    }

    // Returns the worker to Idle after its assigned proof reported a terminal
    // failure (Unexecutable).  A failed proof is not evidence of a broken
    // worker (the request itself may be malformed), so no strike is added and
    // the span-proof average is untouched.
    pub(super) fn failed_proof(&mut self) {
        self.status = WorkerStatus::Idle;
    }

    pub(super) fn add_strike(&mut self) {
        self.strikes += 1;
        debug!("Strike added to worker.  New strikes: {}", self.strikes);
    }

    pub(super) fn assigned_proof(&mut self, request_id: B256, vm: VmKind, mode_name: String) {
        self.status = WorkerStatus::Busy {
            request_id,
            vm,
            mode_name,
            start_time: Instant::now(),
            progress: None,
            time_of_last_update: SystemTime::now(),
        };
        self.strikes = 0;
    }

    pub(super) fn heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    pub(super) fn should_drop(&self, config: &WorkerRegistryConfig) -> bool {
        if self.strikes >= config.max_worker_strikes {
            warn!(
                "Dropping contemplant {} because they have {} strikes",
                self.name, self.strikes
            );
            true
        } else if self.last_heartbeat.elapsed() >= config.max_worker_heartbeat_interval_secs {
            warn!(
                "Dropping contemplant {} because their last heartbeat was {} seconds ago",
                self.name,
                self.last_heartbeat.elapsed().as_secs_f32()
            );
            true
        } else if let WorkerStatus::Busy {
            request_id,
            start_time,
            time_of_last_update,
            progress,
            ..
        } = &self.status
        {
            let mins_on_this_proof = (start_time.elapsed().as_secs_f32() / 60.0) as u64;
            if mins_on_this_proof > config.proof_timeout_mins {
                warn!(
                    "Dropping contemplant {} because they have been working on proof request {} for {} mins.  Max proof time is set to {} mins.",
                    self.name, request_id, mins_on_this_proof, config.proof_timeout_mins
                );
                true
            } else if let Ok(duration_since_last_update) =
                SystemTime::now().duration_since(*time_of_last_update)
            {
                let mins_since_last_update =
                    (duration_since_last_update.as_secs_f64() / 60.0) as u64;
                if config.worker_required_progress_interval_mins > 0
                    && mins_since_last_update > config.worker_required_progress_interval_mins
                {
                    warn!(
                        "Dropping contemplant {} because they haven't made progress on proof {} in {} mins.",
                        self.name, request_id, mins_since_last_update
                    );
                    true
                } else {
                    false
                }
            } else if progress.is_none() {
                if mins_on_this_proof > config.worker_max_execution_report_mins {
                    warn!(
                        "Dropping contemplant {} of proof {} because they've been running the execution report for {} mins.  Max time allowed {} mins.",
                        self.name,
                        request_id,
                        mins_on_this_proof,
                        config.worker_max_execution_report_mins
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    // returns (request_id, vm, mode_name) of the current proof, if any
    pub(super) fn current_proof(&self) -> Option<(B256, VmKind, String)> {
        match &self.status {
            WorkerStatus::Idle => None,
            WorkerStatus::Busy {
                request_id,
                vm,
                mode_name,
                ..
            } => Some((*request_id, *vm, mode_name.clone())),
        }
    }
}

impl Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vms = self
            .capabilities
            .vms
            .iter()
            .map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let groth16 = if self.capabilities.risc0_groth16 {
            ", Groth16"
        } else {
            ""
        };
        let openvm_evm = if self.capabilities.openvm_evm {
            ", OpenVM-EVM"
        } else {
            ""
        };
        write!(
            f,
            "name: {} [VMs: {}{}{}] status: {}, strikes: {}",
            self.name, vms, groth16, openvm_evm, self.status, self.strikes
        )
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Serialize)]
pub enum WorkerStatus {
    Idle,
    Busy {
        request_id: B256,
        vm: VmKind,
        mode_name: String,
        #[serde(rename = "elapsed_secs", serialize_with = "serialize_instant_age_secs")]
        start_time: Instant,
        progress: Option<ProgressUpdate>,
        #[serde(skip_serializing)]
        time_of_last_update: SystemTime,
    },
}

// Serialize an Instant as its whole-seconds age (elapsed since capture), so
// the client gets a machine-readable number to format as it likes rather
// than a lossy pre-rounded human string.
pub fn serialize_instant_age_secs<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_u64(instant.elapsed().as_secs())
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy {
                request_id,
                vm,
                mode_name,
                start_time,
                progress,
                ..
            } => {
                let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;
                let progress = match progress {
                    Some(progress) => {
                        format!("{progress}")
                    }
                    None => "not started".to_string(),
                };
                write!(
                    f,
                    "{vm} {mode_name} proof {request_id} is {progress}. Computing for {minutes} minutes"
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

}
