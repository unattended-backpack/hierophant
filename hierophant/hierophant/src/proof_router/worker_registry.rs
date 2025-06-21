use crate::hierophant_state::ProofStatus;
use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use log::{debug, error, info, trace, warn};
use network_lib::{
    CONTEMPLANT_VERSION, ContemplantProofRequest, ContemplantProofStatus, FromHierophantMessage,
    MagisterInfo, ProgressUpdate, WorkerRegisterInfo,
};
use serde::{Serialize, Serializer};
use sp1_sdk::network::proto::network::ProofMode;
use std::ops::ControlFlow;
use std::time::SystemTime;
use std::{
    collections::HashMap,
    fmt::{self, Display},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, Instant, timeout},
};

#[derive(Clone, Debug)]
pub struct WorkerRegistryClient {
    pub sender: mpsc::Sender<WorkerRegistryCommand>,
}

impl WorkerRegistryClient {
    pub fn new(
        cfg_max_worker_strikes: usize,
        cfg_max_worker_heartbeat_interval_secs: Duration,
        cfg_proof_timeout_mins: u64,
        cfg_contemplant_required_progress_interval_mins: u64,
        cfg_contemplant_max_execution_report_mins: u64,
    ) -> Self {
        let workers = HashMap::new();
        let dead_workers = Vec::new();

        let (sender, receiver) = mpsc::channel(100);

        let awaiting_proof_status_responses = HashMap::new();

        let proof_history = Vec::new();

        let reqwest_client = reqwest::Client::new();

        let worker_registry = WorkerRegistry {
            cfg_max_worker_heartbeat_interval_secs,
            cfg_max_worker_strikes,
            cfg_proof_timeout_mins,
            cfg_contemplant_required_progress_interval_mins,
            cfg_contemplant_max_execution_report_mins,
            workers,
            dead_workers,
            receiver,
            awaiting_proof_status_responses,
            proof_history,
            reqwest_client,
        };

        tokio::task::spawn(async move { worker_registry.background_event_loop().await });

        Self { sender }
    }

    pub async fn heartbeat(&self, worker_addr: String) -> ControlFlow<(), ()> {
        let (sender, receiver) = oneshot::channel();
        if let Err(e) = self
            .sender
            .send(WorkerRegistryCommand::Heartbeat {
                worker_addr,
                should_drop_sender: sender,
            })
            .await
        {
            error!("Failed to send command Heartbeat: {}", e);
        }

        match receiver.await {
            Ok(should_drop) => {
                if should_drop {
                    // tell ws_handler to drop this connection
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }
            Err(e) => {
                // error here isn't a good enough reason to drop the contemplant
                error!("{e}");
                ControlFlow::Continue(())
            }
        }
    }

    pub async fn worker_ready(
        &self,
        worker_addr: String,
        worker_register_info: WorkerRegisterInfo,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) -> ControlFlow<(), ()> {
        // check to make sure the contemplant is on the same version as the hierophant
        if CONTEMPLANT_VERSION != worker_register_info.contemplant_version {
            let error = anyhow!(
                "Contemplant registration denied. Contemplant {} at {} has CONTEMPLANT_VERSION {} but this Hierophant has CONTEMPLANT_VERSION {}.",
                worker_register_info.name,
                worker_addr,
                worker_register_info.contemplant_version,
                CONTEMPLANT_VERSION
            );
            warn!("{error}");
            // end this ws connection
            return ControlFlow::Break(());
        }

        match self
            .sender
            .send(WorkerRegistryCommand::WorkerReady {
                worker_addr,
                worker_name: worker_register_info.name,
                worker_magister: worker_register_info.magister,
                from_hierophant_sender,
            })
            .await
        {
            Ok(_) => ControlFlow::Continue(()),
            Err(e) => {
                error!("Worker registry process ended early. {e}");
                // Program shutting down, end ws connections cleanly
                ControlFlow::Break(())
            }
        }
    }

    pub async fn assign_proof_request(&self, proof_request: ContemplantProofRequest) -> Result<()> {
        self.sender
            .send(WorkerRegistryCommand::AssignProofRequest { proof_request })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command AssignProofRequest: {}", e))
    }

    // response from contemplant about a proof_status we previously requested
    pub async fn proof_status_response(
        &self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) -> ControlFlow<(), ()> {
        match self
            .sender
            .send(WorkerRegistryCommand::ProofStatusResponse {
                request_id,
                maybe_proof_status,
            })
            .await
        {
            Err(e) => {
                error!(
                    "Error {e} sending worker registry command ProofStatusResponse.  Worker registry service likely exited"
                );
                ControlFlow::Break(())
            }
            Ok(_) => ControlFlow::Continue(()),
        }
    }

    pub async fn drop_worker_of_request(&self, request_id: B256) {
        if let Err(e) = self
            .sender
            .send(WorkerRegistryCommand::DropWorkerOfRequest { request_id })
            .await
        {
            error!("{e}");
        }
    }

    // None (no worker has this proof) or Some(ProofStatus)
    // Initiates a request/response pattern with the contemplant over ws (via
    // WorkerRegistryCommand::ProofStatusRequest)
    pub async fn proof_status_request(
        &self,
        request_id: B256,
        response_timeout: Duration,
    ) -> Result<Option<ProofStatus>> {
        let (resp_sender, receiver) = oneshot::channel::<Option<ContemplantProofStatus>>();
        // Under the hood, ProofStatusRequest prompts a request/ response pattern with the
        // contemplant
        self.sender
            .send(WorkerRegistryCommand::ProofStatusRequest {
                target_request_id: request_id,
                resp_sender,
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command ProofStatus: {}", e))?;

        // wait for the response or for the worker timeout
        match timeout(response_timeout, receiver).await {
            Err(_) => {
                // Transient network errors occur when using outsourced gpus, and we
                // should tolerate a few misses
                warn!(
                    "Reached timeout of {} seconds while waiting for a proof_status response for request {}",
                    response_timeout.as_secs_f32(),
                    request_id
                );

                // add strike to the worker working on this request
                self.sender
                    .send(WorkerRegistryCommand::StrikeWorkerOfRequest { request_id })
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to send command StrikeWorkerOfRequest: {}", e)
                    })?;

                // If we returned a ProofStatus::lost here it would prompt the client
                // to re-request the proof.  We want the client to instead re-request
                // the proof_status request, and only re-request the proof when this
                // worker is removed.
                let proof_status = ProofStatus::assigned();
                Ok(Some(proof_status))
            }
            Ok(Err(e)) => {
                // We didn't reach the timeout but the sender was dropped
                // This most likely means our worker_registry service shut down and is unrecoverable
                error!("Worker_registry service might be down. {e}");
                Err(e.into())
            }
            Ok(Ok(maybe_contemplant_proof_status)) => {
                match maybe_contemplant_proof_status {
                    // No worker is assigned to this proof
                    None => Ok(None),
                    // got proof status
                    Some(contemplant_proof_status) => {
                        self.sender
                            .send(WorkerRegistryCommand::ProofProgressUpdate {
                                request_id,
                                progress_update: contemplant_proof_status.progress,
                            })
                            .await
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to send command ProofProgressUpdate: {}", e)
                            })?;

                        let proof_status = contemplant_proof_status.into();
                        Ok(Some(proof_status))
                    }
                }
            }
        }
    }

    // signal that the proposer got the proof and the worker is ready to receive a new proof
    pub async fn proof_complete(&self, request_id: B256) -> Result<()> {
        self.sender
            .send(WorkerRegistryCommand::ProofComplete { request_id })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command ProofComplete: {}", e))
    }

    pub async fn workers(&self) -> Result<Vec<(String, WorkerState)>> {
        let (resp_sender, receiver) = oneshot::channel();
        self.sender
            .send(WorkerRegistryCommand::Workers { resp_sender })
            .await?;

        receiver.await.map_err(|e| anyhow!(e))
    }

    pub async fn dead_workers(&self) -> Result<Vec<(String, WorkerState)>> {
        let (resp_sender, receiver) = oneshot::channel();
        self.sender
            .send(WorkerRegistryCommand::DeadWorkers { resp_sender })
            .await?;

        receiver.await.map_err(|e| anyhow!(e))
    }

    pub async fn proof_history(&self) -> Result<Vec<CompletedProofInfo>> {
        let (resp_sender, receiver) = oneshot::channel();
        self.sender
            .send(WorkerRegistryCommand::ProofHistory { resp_sender })
            .await?;

        receiver.await.map_err(|e| anyhow!(e))
    }
}

pub struct WorkerRegistry {
    pub cfg_max_worker_strikes: usize,
    pub cfg_max_worker_heartbeat_interval_secs: Duration,
    pub cfg_proof_timeout_mins: u64,
    pub cfg_contemplant_required_progress_interval_mins: u64,
    pub cfg_contemplant_max_execution_report_mins: u64,
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
    async fn background_event_loop(mut self) {
        while let Some(command) = self.receiver.recv().await {
            let start = Instant::now();
            let command_string = format!("{:?}", command);
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
                    worker_magister,
                    from_hierophant_sender,
                } => {
                    self.handle_worker_ready(
                        worker_addr,
                        worker_name,
                        worker_magister,
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
                // TODO: remove this or send it to debug.  Just for basic benchmarking
                info!(
                    "Slow execution detected: took {} seconds to process worker_registry command {:?}",
                    secs, command_string
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
                if worker_state.should_drop(
                    self.cfg_max_worker_strikes,
                    self.cfg_max_worker_heartbeat_interval_secs,
                    self.cfg_proof_timeout_mins,
                    self.cfg_contemplant_required_progress_interval_mins,
                    self.cfg_contemplant_max_execution_report_mins,
                ) {
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
                if let Some(magister) = dead_worker_state.magister.clone() {
                    let url = magister.to_drop_url();
                    debug!(
                        "Notifying magister with {url} for worker {dead_worker_state} at {dead_worker_addr}"
                    );
                    let client_clone = self.reqwest_client.clone();
                    // TODO: handle response if we decide the hierophant should care about this call failing
                    tokio::spawn(async move {
                        if let Err(e) = client_clone.delete(url.clone()).send().await {
                            warn!("Error sending drop message to magister {url}: {e}");
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
            debug!("Worker {} state {}", worker_addr, worker_state);

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

            info!(
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
        // which will trigger another AssignProof request here TODO: don't drive on this

        warn!("No workers available for proof {request_id}");
    }

    async fn handle_worker_ready(
        &mut self,
        worker_addr: String,
        worker_name: String,
        worker_magister: Option<MagisterInfo>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) {
        // TODO: should we ping the magister before adding this contemplant?  Or is it the
        // magisters problem that they passed an incorrect address?

        let default_state =
            WorkerState::new(worker_name.clone(), worker_magister, from_hierophant_sender);
        match self
            .workers
            .insert(worker_addr.clone(), default_state.clone())
        {
            Some(old_state) => {
                // if this worker was working on a proof but we didn't drop it
                if old_state.is_busy()
                    && !old_state.should_drop(
                        self.cfg_max_worker_strikes,
                        self.cfg_max_worker_heartbeat_interval_secs,
                        self.cfg_proof_timeout_mins,
                        self.cfg_contemplant_required_progress_interval_mins,
                        self.cfg_contemplant_max_execution_report_mins,
                    )
                {
                    // TODO: re-assign this proof request
                    // Currently, this is resolved when sp1_sdk requests the proof status because
                    // the hierophant will see no worker working on it and will re-assign.  But we
                    // might want it to happen earlier than that.  i.e. We might want to NOT be
                    // driven by proof status requests.
                    error!(
                        "Contemplant {} re-started but wasn't dropped yet.  Contemplant's previous state: {}",
                        worker_addr, old_state
                    );
                } else {
                    info!(
                        "Known contemplant {} at {} re-started, resetting state from {} to {}",
                        worker_name, worker_addr, old_state, default_state
                    );
                }
            }
            None => {
                info!(
                    "New contemplant {} at {} added to registry",
                    worker_name, worker_addr
                );
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
            if let Err(_) = sender.send(maybe_proof_status.clone()) {
                // if the receiver is dropped, it means we reached the timeout before
                // this contemplant responded.  The contemplant was already given
                // a strike for this, so nothing to do here
            }
        }
    }

    fn handle_drop_worker_of_request(&mut self, target_request_id: B256) {
        // get worker assigned to this proof, if any
        let (_, worker_state) =
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
                    info!(
                        "Can't drop worker because no worker is assigned to proof {}",
                        target_request_id
                    );
                    return;
                }
            };

        // set their strikes to the max. This will trigger a drop
        worker_state.strikes = self.cfg_max_worker_strikes;

        // remove this and any other dead workers
        self.trim_workers();
    }

    fn handle_strike_worker_of_request(&mut self, target_request_id: B256) {
        // get worker assigned to this proof, if any
        let (_, worker_state) =
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
                    info!(
                        "Can't strike worker because no worker is assigned to proof {}",
                        target_request_id
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
                    info!("No worker is assigned to proof {}", target_request_id);
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
            worker_state.strikes = self.cfg_max_worker_strikes;
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
            .or_insert_with(Vec::new)
            .push(resp_sender);
    }

    fn handle_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let workers = self
            .workers
            .iter()
            .map(|(x, y)| (x.clone(), y.clone()))
            .collect();
        if let Err(_) = resp_sender.send(workers) {
            warn!("Receiver for WorkerRegistryCommand::Workers dropped");
        }
    }

    fn handle_dead_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let dead_workers = self.dead_workers.clone();
        if let Err(_) = resp_sender.send(dead_workers) {
            warn!("Receiver for WorkerRegistryCommand::DeadWorkers dropped");
        }
    }

    fn handle_proof_history(&self, resp_sender: oneshot::Sender<Vec<CompletedProofInfo>>) {
        let completed_proof_info = self.proof_history.iter().map(|x| x.clone()).collect();
        if let Err(_) = resp_sender.send(completed_proof_info) {
            warn!("Receiver for WorkerRegistryCommand::ProofHistory dropped");
        }
    }
}

pub enum WorkerRegistryCommand {
    AssignProofRequest {
        proof_request: ContemplantProofRequest,
    },
    WorkerReady {
        worker_addr: String,
        worker_name: String,
        worker_magister: Option<MagisterInfo>,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    },
    // sp1_sdk requests the status of a proof
    ProofStatusRequest {
        target_request_id: B256,
        resp_sender: oneshot::Sender<Option<ContemplantProofStatus>>,
    },
    // a contemplant responds with a previously requested proof status
    ProofStatusResponse {
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    },
    ProofComplete {
        request_id: B256,
    },
    ProofProgressUpdate {
        request_id: B256,
        progress_update: Option<ProgressUpdate>,
    },
    Workers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
    DeadWorkers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
    ProofHistory {
        resp_sender: oneshot::Sender<Vec<CompletedProofInfo>>,
    },
    Heartbeat {
        worker_addr: String,
        should_drop_sender: oneshot::Sender<bool>,
    },
    // only used in external (external to workerRegistry state) functions like
    // WorkerRegistryClient.proof_status_request
    StrikeWorkerOfRequest {
        request_id: B256,
    },
    // only used in external (external to workerRegistry state) functions like
    // prover_network_service.get_proof_status (drop worker when they return an invalid proof)
    DropWorkerOfRequest {
        request_id: B256,
    },
}

impl fmt::Debug for WorkerRegistryCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let command = match self {
            WorkerRegistryCommand::AssignProofRequest { .. } => {
                format!("AssignProofRequest")
            }
            WorkerRegistryCommand::WorkerReady { .. } => {
                format!("WorkerReady")
            }
            WorkerRegistryCommand::ProofComplete { .. } => {
                format!("ProofComplete")
            }
            WorkerRegistryCommand::ProofProgressUpdate { .. } => {
                format!("ProofProgressUpdate")
            }
            WorkerRegistryCommand::ProofStatusRequest { .. } => {
                format!("ProofStatusRequest")
            }
            WorkerRegistryCommand::ProofStatusResponse { .. } => {
                format!("ProofStatusResponse")
            }
            WorkerRegistryCommand::Workers { .. } => {
                format!("Workers")
            }
            WorkerRegistryCommand::DeadWorkers { .. } => {
                format!("DeadWorkers")
            }
            WorkerRegistryCommand::ProofHistory { .. } => {
                format!("ProofHistory")
            }
            WorkerRegistryCommand::Heartbeat { .. } => {
                format!("Heartbeat")
            }
            WorkerRegistryCommand::StrikeWorkerOfRequest { .. } => {
                format!("StrikeWorkerOfRequest")
            }
            WorkerRegistryCommand::DropWorkerOfRequest { .. } => {
                format!("DropWorkerOfRequest")
            }
        };
        write!(f, "{command}")
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WorkerState {
    name: String,
    status: WorkerStatus,
    strikes: usize,
    num_completed_span_proofs: usize,
    average_span_proof_time: f32,
    #[serde(skip_serializing)]
    last_heartbeat: Instant,
    #[serde(skip_serializing)]
    from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    magister: Option<MagisterInfo>,
}

impl WorkerState {
    fn new(
        name: String,
        magister: Option<MagisterInfo>,
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
            magister,
        }
    }
    fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
    }

    fn completed_proof(&mut self, minutes_to_complete: f32) {
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

    fn add_strike(&mut self) {
        self.strikes += 1;
        debug!("Strike added to worker.  New strikes: {}", self.strikes);
    }

    fn assigned_proof(&mut self, request_id: B256, proof_mode: ProofMode) {
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

    fn heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    // drop worker if they have too many strikes OR
    // if it's been too long since their last heartbeat OR
    // if they've been working on a proof for too long OR
    // if it's been too long since their proof made progress
    fn should_drop(
        &self,
        cfg_max_worker_strikes: usize,
        cfg_max_worker_heartbeat_interval_secs: Duration,
        cfg_proof_timeout_mins: u64,
        cfg_contemplant_required_progress_interval_mins: u64,
        cfg_contemplant_max_execution_report_mins: u64,
    ) -> bool {
        if self.strikes >= cfg_max_worker_strikes {
            warn!(
                "Dropping contemplant {} because they have {} strikes",
                self.name, self.strikes
            );
            true
        } else if self.last_heartbeat.elapsed() >= cfg_max_worker_heartbeat_interval_secs {
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
            if mins_on_this_proof > cfg_proof_timeout_mins {
                warn!(
                    "Dropping contemplant {} because they have been working on proof request {} for {} mins.  Max proof time is set to {} mins.",
                    self.name, request_id, mins_on_this_proof, cfg_proof_timeout_mins
                );
                true
            } else if let Ok(duration_since_last_update) =
                SystemTime::now().duration_since(time_of_last_update)
            {
                let mins_since_last_update =
                    (duration_since_last_update.as_secs_f64() / 60.0) as u64;
                // if it's been too long since this contemplant has reported progress on this proof
                if mins_since_last_update > cfg_contemplant_required_progress_interval_mins {
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
                if mins_on_this_proof > cfg_contemplant_max_execution_report_mins {
                    warn!(
                        "Dropping contemplant {} of proof {} because they've been running the execution report for {} mins.  Max time allowed {} mins.",
                        self.name,
                        request_id,
                        mins_on_this_proof,
                        cfg_contemplant_max_execution_report_mins
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
    fn current_proof(&self) -> Option<(B256, ProofMode)> {
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

#[derive(Serialize, Debug, Clone)]
pub struct CompletedProofInfo {
    proof_request_id: B256,
    proof_mode: String,
    worker_addr: String,
    worker_name: String,
    minutes_to_complete: f32,
}

impl CompletedProofInfo {
    pub fn new(
        proof_request_id: B256,
        proof_mode: ProofMode,
        minutes_to_complete: f32,
        worker_addr: String,
        worker_name: String,
    ) -> Self {
        Self {
            proof_request_id,
            proof_mode: proof_mode.as_str_name().into(),
            minutes_to_complete,
            worker_name,
            worker_addr,
        }
    }
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
