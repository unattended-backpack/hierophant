use crate::config::WorkerRegistryConfig;
use alloy_primitives::B256;
use anyhow::Result;
use log::{debug, warn};
use network_lib::{ProgressUpdate, messages::FromHierophantMessage};
use serde::{Serialize, Serializer};
use sp1_sdk::network::proto::network::ProofMode;
use std::fmt::Display;
use std::time::SystemTime;
use tokio::{sync::mpsc, time::Instant};

#[derive(Clone, Debug, Serialize)]
pub struct WorkerState {
    pub name: String,
    pub status: WorkerStatus,
    pub strikes: usize,
    pub num_completed_span_proofs: usize,
    pub average_span_proof_time: f32,
    #[serde(skip_serializing)]
    pub last_heartbeat: Instant,
    #[serde(skip_serializing)]
    pub from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    pub magister_drop_endpoint: Option<String>,
}

impl WorkerState {
    pub(super) fn new(
        name: String,
        magister_drop_endpoint: Option<String>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) -> Self {
        Self {
            name,
            status: WorkerStatus::Idle,
            strikes: 0,
            num_completed_span_proofs: 0,
            average_span_proof_time: 0.0,
            last_heartbeat: Instant::now(),
            from_hierophant_sender,
            magister_drop_endpoint,
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
    }

    pub(super) fn completed_proof(&mut self, minutes_to_complete: f32) {
        match self.status {
            // it is never idle if we get here
            WorkerStatus::Idle => (),
            WorkerStatus::Busy { proof_mode, .. } => {
                // if it was a span proof, add it to the average
                if let ProofMode::Compressed = proof_mode {
                    let n = self.num_completed_span_proofs as f32 + 1.0;
                    let old_average = self.average_span_proof_time;
                    let new_element = minutes_to_complete;

                    let new_average = add_to_average(n, old_average, new_element);

                    self.average_span_proof_time = new_average;
                    self.num_completed_span_proofs += 1;
                }
            }
        };

        // set them to idle
        self.status = WorkerStatus::Idle;
        // they've been good, reset their proofs
        self.strikes = 0;
    }

    pub(super) fn add_strike(&mut self) {
        self.strikes += 1;
        debug!("Strike added to worker.  New strikes: {}", self.strikes);
    }

    pub(super) fn assigned_proof(&mut self, request_id: B256, proof_mode: ProofMode) {
        self.status = WorkerStatus::Busy {
            request_id,
            proof_mode,
            start_time: Instant::now(),
            progress: None,
            time_of_last_update: SystemTime::now(),
        };
        // This worker has been good.  Reset their strikes
        self.strikes = 0;
    }

    pub(super) fn heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    // drop worker if they have too many strikes OR
    // if it's been too long since their last heartbeat OR
    // if they've been working on a proof for too long OR
    // if it's been too long since their proof made progress
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
        } = self.status
        {
            let mins_on_this_proof = (start_time.elapsed().as_secs_f32() / 60.0) as u64;
            // if they've been working on this proof for too long
            if mins_on_this_proof > config.proof_timeout_mins {
                warn!(
                    "Dropping contemplant {} because they have been working on proof request {} for {} mins.  Max proof time is set to {} mins.",
                    self.name, request_id, mins_on_this_proof, config.proof_timeout_mins
                );
                true
            } else if let Ok(duration_since_last_update) =
                SystemTime::now().duration_since(time_of_last_update)
            {
                let mins_since_last_update =
                    (duration_since_last_update.as_secs_f64() / 60.0) as u64;
                // if it's been too long since this contemplant has reported progress on this proof
                if mins_since_last_update > config.worker_required_progress_interval_mins {
                    warn!(
                        "Dropping contemplant {} because they haven't made progress on proof {} in {} mins.",
                        self.name, request_id, mins_since_last_update
                    );
                    true
                } else {
                    false
                }
            } else if let None = progress {
                // progress starts as None and moves to Some when the execution report is done
                // and the proof starts executing.  Progress never moves from Some to None.
                // If the contemplant takes too long on the execution report, drop them.
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

    // returns the proof it's currently working on, if any
    pub(super) fn current_proof(&self) -> Option<(B256, ProofMode)> {
        match self.status {
            WorkerStatus::Idle => None,
            WorkerStatus::Busy {
                request_id,
                proof_mode,
                ..
            } => Some((request_id, proof_mode)),
        }
    }
}

impl Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "name: {} status: {}, strikes: {}",
            self.name, self.status, self.strikes
        )
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Serialize)]
pub enum WorkerStatus {
    Idle,
    Busy {
        request_id: B256,
        proof_mode: ProofMode,
        #[serde(serialize_with = "serialize_instant_as_minutes")]
        start_time: Instant,
        progress: Option<ProgressUpdate>,
        #[serde(skip_serializing)]
        time_of_last_update: SystemTime,
    },
}

pub fn serialize_instant_as_minutes<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let minutes_elapsed = (instant.elapsed().as_secs_f64() / 60.0).round() as u32;
    let minutes_elapsed = format!("{minutes_elapsed} minutes ago");
    serializer.serialize_str(&minutes_elapsed)
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy {
                request_id,
                proof_mode,
                start_time,
                progress,
                ..
            } => {
                let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;
                let progress = match progress {
                    Some(progress) => {
                        format!("{progress}")
                    }
                    None => {
                        format!("not started")
                    }
                };
                write!(
                    f,
                    "{} proof {request_id} is {progress}. Computing for {minutes} minutes",
                    proof_mode.as_str_name()
                )
            }
        }
    }
}

// where n is the new number of elements
fn add_to_average(n: f32, old_average: f32, new_element: f32) -> f32 {
    old_average + ((new_element - old_average) / n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_to_average() {
        let a = add_to_average(3.0, 5.0, 5.0);
        assert_eq!(a, 5.0);

        let a = add_to_average(2.0, 1.0, 0.0);
        assert_eq!(a, 0.5);

        let a = add_to_average(4.0, 6.0, 4.0);
        assert_eq!(a, 5.5);
    }
}
