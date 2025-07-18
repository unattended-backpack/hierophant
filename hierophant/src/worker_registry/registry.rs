use super::command::WorkerRegistryCommand;
use super::worker_state::{WorkerState, WorkerStatus};
use crate::config::WorkerRegistryConfig;
use crate::proof::CompletedProofInfo;

use alloy_primitives::B256;
use log::{debug, error, info, trace, warn};
use network_lib::{
    ContemplantProofRequest, ContemplantProofStatus, ProgressUpdate,
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
                    magister_drop_endpoint,
                    from_hierophant_sender,
                } => {
                    self.handle_worker_ready(
                        worker_addr,
                        worker_name,
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

    // iterate through workers and remove any who have
    // strikes > cfg_max_worker_strikes,
    // last_hearbeat > cfg_max_worker_heartbeat_interval_secs,
    // or proof_time > proof_timeout_mins
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
            // remove them from the mapping
            if let Some(dead_worker_state) = self.workers.remove(&dead_worker_addr) {
                info!(
                    "Removing worker {} at {dead_worker_addr} from worker registry",
                    dead_worker_state.name
                );

                if let Some((dangling_proof, dangling_proof_mode)) =
                    dead_worker_state.current_proof()
                {
                    // The dangling proof will eventually be requested for by the proposer via
                    // `proof_status` and the registry will return None, which will cause the
                    // coordinator to return `ProofStatus::lost()`, which will cause the proposer
                    // to re-request the proof

                    warn!(
                        "{} proof {} left incomplete as a result of killing worker {} at {dead_worker_addr}",
                        dangling_proof_mode.as_str_name(),
                        dangling_proof,
                        dead_worker_state.name
                    );
                }

                // Notify the worker's magister so they can be deallocated
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

                // add them to the list of dead workers
                self.dead_workers
                    .push((dead_worker_addr, dead_worker_state));
            }
        }
    }

    // updates worker's last heartbeat
    // if the worker doesn't exist then we order ws_handler to drop the connection
    fn handle_heartbeat(&mut self, worker_addr: String, should_drop_sender: oneshot::Sender<bool>) {
        let should_drop = match self.workers.get_mut(&worker_addr) {
            Some(worker) => {
                // re-set last heartbeat
                worker.heartbeat();
                false
            }
            // received a heartbeat from a worker we evicted.  They're still connected by ws so we
            // should drop them
            None => true,
        };

        // does what it says on the can
        let _ = should_drop_sender.send(should_drop);
    }

    async fn handle_assign_proof(&mut self, proof_request: &ContemplantProofRequest) {
        let request_id = proof_request.request_id;
        // remove any dead workers
        self.trim_workers();

        // iterate over all idle workers
        for (worker_addr, worker_state) in self.workers.iter_mut() {
            debug!("Worker {worker_addr} state {worker_state}");

            // skip a worker if it's busy or return early if there's already a worker proving this
            if let WorkerStatus::Busy {
                request_id: workers_request_id,
                ..
            } = worker_state.status
            {
                if workers_request_id == request_id {
                    info!(
                        "Received proof request for {} proof {} but worker {} is already busy with it",
                        proof_request.mode.as_str_name(),
                        request_id,
                        worker_addr
                    );
                    // there's already a worker proving this.  We can return
                    // early
                    return;
                } else {
                    // can skip workers who are already working on a proof
                    continue;
                }
            }

            debug!(
                "Attemping to assign proof request {request_id} to worker {} at {worker_addr}",
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
                    error!("Error sending proof request {request_id} to worker {worker_addr}: {e}");
                    worker_state.add_strike();
                }
                Ok(_) => {
                    info!(
                        "Proof request {request_id} assigned to worker {} at {worker_addr}",
                        worker_state.name
                    );
                    worker_state.assigned_proof(request_id, proof_request.mode);
                    // assigned to a worker successfully, return
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

        warn!("No workers available for proof {request_id}");
    }

    async fn handle_worker_ready(
        &mut self,
        worker_addr: String,
        worker_name: String,
        magister_drop_endpoint: Option<String>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) {
        let default_state = WorkerState::new(
            worker_name.clone(),
            magister_drop_endpoint,
            from_hierophant_sender,
        );
        match self
            .workers
            .insert(worker_addr.clone(), default_state.clone())
        {
            Some(old_state) => {
                // if this worker was working on a proof but we didn't drop it
                if old_state.is_busy() && !old_state.should_drop(&self.config) {
                    // TODO: re-assign this proof request
                    // Currently, this is resolved when sp1_sdk requests the proof status because
                    // the hierophant will see no worker working on it and will re-assign.  But we
                    // might want it to happen earlier than that.  i.e. We might want to NOT be
                    // driven by proof status requests.
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
            if let Some((id, _)) = worker_state.current_proof() {
                id == request_id
            } else {
                false
            }
        }) {
            // update the worker's state with the progress
            if let WorkerStatus::Busy {
                progress,
                time_of_last_update,
                ..
            } = &mut worker_state.status
            {
                match progress_update {
                    Some(_) => {
                        // if progress has been made, update time_of_last_update and this
                        // contemplant's progress
                        if progress_update > *progress {
                            *time_of_last_update = SystemTime::now();
                            *progress = progress_update;
                        }
                    }
                    None => {
                        // We don't keep track of time between updates if the contemplant
                        // hasn't started executing the proof (i.e. hasn't finished the
                        // execution report yet and progress_update is None)
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
                if let Some((id, _)) = worker_state.current_proof() {
                    id == request_id
                } else {
                    false
                }
            })
        {
            if let WorkerStatus::Busy {
                request_id: busy_request_id,
                proof_mode,
                start_time,
                ..
            } = worker_state.status
            {
                // if they're marked as busy with the proof we just saw completed
                if busy_request_id == request_id {
                    // We know the worker is done with this proof because it just
                    // returned us an executed proof.

                    let minutes_to_complete = start_time.elapsed().as_secs_f32() / 60.0;
                    worker_state.completed_proof(minutes_to_complete);
                    info!(
                        "Worker {} at {} completed a {} proof in {} minutes and is now Idle.",
                        worker_state.name,
                        worker_addr,
                        proof_mode.as_str_name(),
                        minutes_to_complete
                    );

                    // record this completed proof
                    let completed_proof_info = CompletedProofInfo::new(
                        request_id,
                        proof_mode,
                        minutes_to_complete,
                        worker_addr.clone(),
                        worker_state.name.clone(),
                    );
                    self.proof_history.push(completed_proof_info);
                }
            }
        } else {
            warn!("Worker registry couldn't find worker who was assigned proof {request_id}");
        }
    }

    // forwards proof status to the thread awaiting it
    async fn handle_proof_status_response(
        &mut self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) {
        // remove the senders from this mapping and send the proof status to the threads awaiting it
        let tasks_awaiting = match self.awaiting_proof_status_responses.remove(&request_id) {
            Some(s) => s,
            None => {
                return;
            }
        };

        // send the proof status to all tasks waiting for it
        for sender in tasks_awaiting {
            if sender.send(maybe_proof_status.clone()).is_err() {
                // if the receiver is dropped, it means we reached the timeout before
                // this contemplant responded.  The contemplant was already given
                // a strike for this, so nothing to do here
            }
        }
    }

    fn handle_drop_worker_of_request(&mut self, target_request_id: B256) {
        // get worker assigned to this proof, if any
        let (_, worker_state) = match self.workers.iter_mut().find(|(_, worker_state)| {
            match worker_state.status {
                WorkerStatus::Idle => false,
                WorkerStatus::Busy { request_id, .. } => request_id == target_request_id,
            }
        }) {
            Some(worker_assigned) => worker_assigned,
            None => {
                // This proof wasn't assigned to any worker, return none
                info!(
                    "Can't drop worker because no worker is assigned to proof {target_request_id}"
                );
                return;
            }
        };

        // set their strikes to the max. This will trigger a drop
        worker_state.strikes = self.config.max_worker_strikes;

        // remove this and any other dead workers
        self.trim_workers();
    }

    fn handle_strike_worker_of_request(&mut self, target_request_id: B256) {
        // get worker assigned to this proof, if any
        let (_, worker_state) = match self.workers.iter_mut().find(|(_, worker_state)| {
            match worker_state.status {
                WorkerStatus::Idle => false,
                WorkerStatus::Busy { request_id, .. } => request_id == target_request_id,
            }
        }) {
            Some(worker_assigned) => worker_assigned,
            None => {
                // This proof wasn't assigned to any worker, return none
                info!(
                    "Can't strike worker because no worker is assigned to proof {target_request_id}"
                );
                return;
            }
        };

        worker_state.add_strike();

        // remove any dead workers
        self.trim_workers();
    }

    // sends a command to get proof status to a contemplant over ws
    async fn handle_proof_status_request(
        &mut self,
        target_request_id: B256,
        resp_sender: oneshot::Sender<Option<ContemplantProofStatus>>,
    ) {
        // remove any dead workers
        self.trim_workers();
        // get worker assigned to this proof, if any
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
                    // This proof wasn't assigned to any worker, return none
                    info!("No worker is assigned to proof {target_request_id}");
                    let _ = resp_sender.send(None);
                    return;
                }
            };

        info!(
            "Worker {} at {} status: {}",
            worker_state.name, worker_addr, worker_state.status
        );

        // send proof status request to the contemplant working on this proof
        if let Err(e) = worker_state
            .from_hierophant_sender
            .send(FromHierophantMessage::ProofStatusRequest(target_request_id))
            .await
        {
            // The receiving end of this was dropped because the ws was dropped.
            // This means we're no longer connected to this contemplant.  It'll get cleaned up by
            // the `trim_workers()` task.
            worker_state.strikes = self.config.max_worker_strikes;
            // TODO: proof re-assignment if this worker was in the middle of the proof
            warn!(
                "No longer connected to worker {} at {} who was working on proof {target_request_id} (error {e})",
                worker_state.name, worker_addr
            );
            let _ = resp_sender.send(None);
            return;
        }

        // add to mapping so we can send a response via resp_sender when we hear back from the
        // contemplant
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
