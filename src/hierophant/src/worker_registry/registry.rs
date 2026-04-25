use super::command::WorkerRegistryCommand;
use super::worker_state::{WorkerState, WorkerStatus};
use crate::config::WorkerRegistryConfig;
use crate::proof::CompletedProofInfo;

use alloy_primitives::B256;
use log::{debug, error, info, trace, warn};
use network_lib::{
    ContemplantProofRequest, ContemplantProofStatus, ProgressUpdate, VmKind,
    messages::FromHierophantMessage,
};
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

pub(super) struct WorkerRegistry {
    pub config: WorkerRegistryConfig,
    // Using a HashMap is a fine complexity tradeoff because we'll never have >20 workers, so
    // iterating isn't horrible in reality.
    pub workers: HashMap<String, WorkerState>,
    // (ip, state when died)
    pub dead_workers: Vec<(String, WorkerState)>,
    pub receiver: mpsc::Receiver<WorkerRegistryCommand>,
    // mapping for currently outbound proof status requests that are being awaited in a thread
    pub awaiting_proof_status_responses:
        HashMap<B256, Vec<oneshot::Sender<Option<ContemplantProofStatus>>>>,
    // history of compelted proofs and information about the contemplant who completed it
    pub proof_history: Vec<CompletedProofInfo>,
    pub reqwest_client: reqwest::Client,
}

impl WorkerRegistry {
    pub(super) async fn background_event_loop(mut self) {
        while let Some(command) = self.receiver.recv().await {
            let start = Instant::now();
            let command_string = format!("{command:?}");
            trace!(
                "{} messages in worker registry channel",
                self.receiver.len()
            );
            match command {
                WorkerRegistryCommand::AssignProofRequest { ref proof_request } => {
                    self.handle_assign_proof(proof_request).await;
                }
                WorkerRegistryCommand::WorkerReady {
                    worker_addr,
                    worker_name,
                    supported_vms,
                    groth16_enabled,
                    magister_drop_endpoint,
                    from_hierophant_sender,
                } => {
                    self.handle_worker_ready(
                        worker_addr,
                        worker_name,
                        supported_vms,
                        groth16_enabled,
                        magister_drop_endpoint,
                        from_hierophant_sender,
                    )
                    .await;
                }
                WorkerRegistryCommand::ProofComplete { request_id } => {
                    self.handle_proof_complete(request_id).await;
                }
                WorkerRegistryCommand::ProofProgressUpdate {
                    request_id,
                    progress_update,
                } => {
                    self.handle_proof_progress_update(request_id, progress_update)
                        .await;
                }
                WorkerRegistryCommand::ProofStatusResponse {
                    request_id,
                    maybe_proof_status,
                } => {
                    self.handle_proof_status_response(request_id, maybe_proof_status)
                        .await;
                }
                WorkerRegistryCommand::ProofStatusRequest {
                    target_request_id,
                    resp_sender,
                } => {
                    self.handle_proof_status_request(target_request_id, resp_sender)
                        .await;
                }
                WorkerRegistryCommand::Workers { resp_sender } => {
                    self.handle_workers(resp_sender);
                }
                WorkerRegistryCommand::DeadWorkers { resp_sender } => {
                    self.handle_dead_workers(resp_sender);
                }
                WorkerRegistryCommand::ProofHistory { resp_sender } => {
                    self.handle_proof_history(resp_sender);
                }
                WorkerRegistryCommand::Heartbeat {
                    worker_addr,
                    should_drop_sender,
                } => {
                    self.handle_heartbeat(worker_addr, should_drop_sender);
                }
                WorkerRegistryCommand::StrikeWorkerOfRequest { request_id } => {
                    self.handle_strike_worker_of_request(request_id);
                }
                WorkerRegistryCommand::DropWorkerOfRequest { request_id } => {
                    self.handle_drop_worker_of_request(request_id);
                }
            };

            let secs = start.elapsed().as_secs_f64();

            if secs > 0.5 {
                info!(
                    "Slow execution detected: took {secs} seconds to process worker_registry command {command_string:?}"
                );
            }
        }
    }

    fn trim_workers(&mut self) {
        let new_dead_workers: Vec<String> = self
            .workers
            .iter_mut()
            .filter_map(|(worker_addr, worker_state)| {
                if worker_state.should_drop(&self.config) {
                    Some(worker_addr.clone())
                } else {
                    None
                }
            })
            .collect();

        for dead_worker_addr in new_dead_workers {
            if let Some(dead_worker_state) = self.workers.remove(&dead_worker_addr) {
                info!(
                    "Removing worker {} at {dead_worker_addr} from worker registry",
                    dead_worker_state.name
                );

                if let Some((dangling_proof, vm, mode_name)) = dead_worker_state.current_proof() {
                    // The dangling proof will eventually be requested for by the proposer via
                    // `proof_status` and the registry will return None, which will cause the
                    // coordinator to return `ProofStatus::lost()`, which will cause the proposer
                    // to re-request the proof
                    warn!(
                        "{vm} {mode_name} proof {dangling_proof} left incomplete as a result of killing worker {} at {dead_worker_addr}",
                        dead_worker_state.name
                    );
                }

                if let Some(drop_endpoint) = dead_worker_state.magister_drop_endpoint.clone() {
                    debug!(
                        "Notifying Magister to drop worker {dead_worker_state} at {dead_worker_addr} with endpoint {drop_endpoint}"
                    );
                    let client_clone = self.reqwest_client.clone();
                    // TODO: retry this request on a failure
                    tokio::spawn(async move {
                        if let Err(e) = client_clone.delete(drop_endpoint.clone()).send().await {
                            warn!("Error sending drop message to Magister {drop_endpoint}: {e}");
                        }
                    });
                }

                self.dead_workers
                    .push((dead_worker_addr, dead_worker_state));
            }
        }
    }

    fn handle_heartbeat(&mut self, worker_addr: String, should_drop_sender: oneshot::Sender<bool>) {
        let should_drop = match self.workers.get_mut(&worker_addr) {
            Some(worker) => {
                worker.heartbeat();
                false
            }
            None => true,
        };

        let _ = should_drop_sender.send(should_drop);
    }

    async fn handle_assign_proof(&mut self, proof_request: &ContemplantProofRequest) {
        let request_id = proof_request.request_id();
        let target_vm = proof_request.vm();
        let mode_name = proof_request.mode_name();
        let needs_groth16 = proof_request.needs_groth16();
        // remove any dead workers
        self.trim_workers();

        // iterate over all workers, filtered to those that can serve this
        // specific request (right VM + Groth16 capability when needed).
        for (worker_addr, worker_state) in self.workers.iter_mut() {
            debug!("Worker {worker_addr} state {worker_state}");

            if !worker_state.can_serve(proof_request) {
                continue;
            }

            // skip a worker if it's busy or return early if there's already a worker proving this
            if let WorkerStatus::Busy {
                request_id: workers_request_id,
                ..
            } = worker_state.status
            {
                if workers_request_id == request_id {
                    info!(
                        "Received proof request for {target_vm} {mode_name} proof {request_id} but worker {worker_addr} is already busy with it"
                    );
                    return;
                } else {
                    continue;
                }
            }

            debug!(
                "Attemping to assign {target_vm} proof request {request_id} to worker {} at {worker_addr}",
                worker_state.name
            );

            let from_hierophant_message =
                FromHierophantMessage::ProofRequest(proof_request.clone());
            match worker_state
                .from_hierophant_sender
                .send(from_hierophant_message)
                .await
            {
                Err(e) => {
                    error!(
                        "Error sending proof request {request_id} to worker {worker_addr}: {e}"
                    );
                    worker_state.add_strike();
                }
                Ok(_) => {
                    info!(
                        "{target_vm} {mode_name} proof request {request_id} assigned to worker {} at {worker_addr}",
                        worker_state.name
                    );
                    worker_state.assigned_proof(request_id, target_vm, mode_name.clone());
                    return;
                }
            }
        }
        // We iterated through all the workers and couldn't find an idle one who could
        // receive the request.
        //
        // This doesn't result in deadlock because the proposer will call `/status/request_id`
        // which will trigger another AssignProof request here
        // TODO: don't drive on proof status requests

        let requirement = if needs_groth16 {
            format!("{target_vm}-capable Groth16-enabled")
        } else {
            format!("{target_vm}-capable")
        };
        warn!("No {requirement} idle workers available for proof {request_id}");
    }

    async fn handle_worker_ready(
        &mut self,
        worker_addr: String,
        worker_name: String,
        supported_vms: Vec<VmKind>,
        groth16_enabled: bool,
        magister_drop_endpoint: Option<String>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) {
        let default_state = WorkerState::new(
            worker_name.clone(),
            supported_vms,
            groth16_enabled,
            magister_drop_endpoint,
            from_hierophant_sender,
        );
        match self
            .workers
            .insert(worker_addr.clone(), default_state.clone())
        {
            Some(old_state) => {
                if old_state.is_busy() && !old_state.should_drop(&self.config) {
                    // TODO: re-assign this proof request
                    error!(
                        "Contemplant {worker_addr} re-started but wasn't dropped yet.  Contemplant's previous state: {old_state}"
                    );
                } else {
                    info!(
                        "Known contemplant {worker_name} at {worker_addr} re-started, resetting state from {old_state} to {default_state}"
                    );
                }
            }
            None => {
                info!("New contemplant {worker_name} at {worker_addr} added to registry");
            }
        }
    }

    async fn handle_proof_progress_update(
        &mut self,
        request_id: B256,
        progress_update: Option<ProgressUpdate>,
    ) {
        if let Some((_, worker_state)) = self.workers.iter_mut().find(|(_, worker_state)| {
            if let Some((id, ..)) = worker_state.current_proof() {
                id == request_id
            } else {
                false
            }
        }) {
            if let WorkerStatus::Busy {
                progress,
                time_of_last_update,
                ..
            } = &mut worker_state.status
            {
                match progress_update {
                    Some(_) => {
                        if progress_update > *progress {
                            *time_of_last_update = SystemTime::now();
                            *progress = progress_update;
                        }
                    }
                    None => {
                        *time_of_last_update = SystemTime::now();
                    }
                }
            }
        } else {
            warn!("Worker registry couldn't find worker who was assigned proof {request_id}");
        }
    }

    async fn handle_proof_complete(&mut self, request_id: B256) {
        if let Some((worker_addr, worker_state)) =
            self.workers.iter_mut().find(|(_, worker_state)| {
                if let Some((id, ..)) = worker_state.current_proof() {
                    id == request_id
                } else {
                    false
                }
            })
        {
            if let WorkerStatus::Busy {
                request_id: busy_request_id,
                vm,
                mode_name,
                start_time,
                ..
            } = worker_state.status.clone()
            {
                if busy_request_id == request_id {
                    let minutes_to_complete = start_time.elapsed().as_secs_f32() / 60.0;
                    // clone state we need out before mutating worker_state
                    let worker_name = worker_state.name.clone();
                    worker_state.completed_proof(minutes_to_complete);
                    info!(
                        "Worker {worker_name} at {worker_addr} completed a {vm} {mode_name} proof in {minutes_to_complete} minutes and is now Idle."
                    );

                    let completed_proof_info = CompletedProofInfo::new(
                        request_id,
                        vm,
                        mode_name,
                        minutes_to_complete,
                        worker_addr.clone(),
                        worker_name,
                    );
                    self.proof_history.push(completed_proof_info);
                }
            }
        } else {
            warn!("Worker registry couldn't find worker who was assigned proof {request_id}");
        }
    }

    async fn handle_proof_status_response(
        &mut self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) {
        let tasks_awaiting = match self.awaiting_proof_status_responses.remove(&request_id) {
            Some(s) => s,
            None => {
                return;
            }
        };

        for sender in tasks_awaiting {
            if sender.send(maybe_proof_status.clone()).is_err() {
                // if the receiver is dropped, it means we reached the timeout before
                // this contemplant responded.  The contemplant was already given
                // a strike for this, so nothing to do here
            }
        }
    }

    fn handle_drop_worker_of_request(&mut self, target_request_id: B256) {
        let (_, worker_state) = match self.workers.iter_mut().find(|(_, worker_state)| {
            match worker_state.status {
                WorkerStatus::Idle => false,
                WorkerStatus::Busy { request_id, .. } => request_id == target_request_id,
            }
        }) {
            Some(worker_assigned) => worker_assigned,
            None => {
                info!(
                    "Can't drop worker because no worker is assigned to proof {target_request_id}"
                );
                return;
            }
        };

        worker_state.strikes = self.config.max_worker_strikes;

        self.trim_workers();
    }

    fn handle_strike_worker_of_request(&mut self, target_request_id: B256) {
        let (_, worker_state) = match self.workers.iter_mut().find(|(_, worker_state)| {
            match worker_state.status {
                WorkerStatus::Idle => false,
                WorkerStatus::Busy { request_id, .. } => request_id == target_request_id,
            }
        }) {
            Some(worker_assigned) => worker_assigned,
            None => {
                info!(
                    "Can't strike worker because no worker is assigned to proof {target_request_id}"
                );
                return;
            }
        };

        worker_state.add_strike();

        self.trim_workers();
    }

    async fn handle_proof_status_request(
        &mut self,
        target_request_id: B256,
        resp_sender: oneshot::Sender<Option<ContemplantProofStatus>>,
    ) {
        self.trim_workers();
        let (worker_addr, worker_state) =
            match self
                .workers
                .iter_mut()
                .find(|(_, worker_state)| match worker_state.status {
                    WorkerStatus::Idle => false,
                    WorkerStatus::Busy { request_id, .. } => request_id == target_request_id,
                }) {
                Some(worker_assigned) => worker_assigned,
                None => {
                    info!("No worker is assigned to proof {target_request_id}");
                    let _ = resp_sender.send(None);
                    return;
                }
            };

        info!(
            "Worker {} at {} status: {}",
            worker_state.name, worker_addr, worker_state.status
        );

        if let Err(e) = worker_state
            .from_hierophant_sender
            .send(FromHierophantMessage::ProofStatusRequest(target_request_id))
            .await
        {
            worker_state.strikes = self.config.max_worker_strikes;
            // TODO: proof re-assignment if this worker was in the middle of the proof
            warn!(
                "No longer connected to worker {} at {} who was working on proof {target_request_id} (error {e})",
                worker_state.name, worker_addr
            );
            let _ = resp_sender.send(None);
            return;
        }

        self.awaiting_proof_status_responses
            .entry(target_request_id)
            .or_default()
            .push(resp_sender);
    }

    fn handle_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let workers = self
            .workers
            .iter()
            .map(|(x, y)| (x.clone(), y.clone()))
            .collect();
        if resp_sender.send(workers).is_err() {
            warn!("Receiver for WorkerRegistryCommand::Workers dropped");
        }
    }

    fn handle_dead_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let dead_workers = self.dead_workers.clone();
        if resp_sender.send(dead_workers).is_err() {
            warn!("Receiver for WorkerRegistryCommand::DeadWorkers dropped");
        }
    }

    fn handle_proof_history(&self, resp_sender: oneshot::Sender<Vec<CompletedProofInfo>>) {
        let completed_proof_info = self.proof_history.clone();
        if resp_sender.send(completed_proof_info).is_err() {
            warn!("Receiver for WorkerRegistryCommand::ProofHistory dropped");
        }
    }
}
