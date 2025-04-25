use crate::hierophant_state::ProofStatus;
use crate::network::{ExecutionStatus, FulfillmentStatus};
use crate::proof_router::request_with_retries;
use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use hyper::header::ACCESS_CONTROL_EXPOSE_HEADERS;
use log::{debug, error, info, trace, warn};
use network_lib::{
    CONTEMPLANT_VERSION, ContemplantProofRequest, ContemplantProofStatus, FromHierophantMessage,
    WorkerRegisterInfo,
};
use reqwest::Client;
use sp1_sdk::network::proto::network::ProofMode;
use std::ops::ControlFlow;
use std::{
    collections::HashMap,
    fmt::{self, Display},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct WorkerRegistryClient {
    pub sender: mpsc::Sender<WorkerRegistryCommand>,
}

impl WorkerRegistryClient {
    pub fn new(
        cfg_max_worker_strikes: usize,
        cfg_max_worker_heartbeat_interval_secs: Duration,
    ) -> Self {
        let workers = HashMap::new();
        let reqwest_client = Client::new();

        let (sender, receiver) = mpsc::channel(100);

        let worker_registry = WorkerRegistry {
            cfg_max_worker_heartbeat_interval_secs,
            cfg_max_worker_strikes,
            workers,
            reqwest_client,
            receiver,
        };

        tokio::task::spawn(async move { worker_registry.background_event_loop().await });

        Self { sender }
    }

    pub async fn heartbeat(&self, worker_addr: String) -> ControlFlow<(), ()> {
        let (sender, receiver) = oneshot::channel();
        self.sender.send(WorkerRegistryCommand::Heartbeat {
            worker_addr,
            should_drop_sender: sender,
        });

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

    // run until we get None (no worker has this proof) or Some(Ok(ProofStatus)).
    // Each run it potentially trims naughty workers
    pub async fn proof_status(&self, request_id: B256) -> Result<Option<ProofStatus>> {
        let mut a_worker_is_assigned = false;
        loop {
            let (resp_sender, receiver) = oneshot::channel();
            self.sender
                .send(WorkerRegistryCommand::ProofStatus {
                    target_request_id: request_id,
                    resp_sender,
                })
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send command ProofStatus: {}", e))?;

            match receiver.await? {
                // worker_registry doesn't have any worker assigned to this proof
                None => {
                    if a_worker_is_assigned {
                        error!(
                            "Worker assigned to proof {request_id} was kicked from the worker registry."
                        );
                        // we know from a previous response that there was a worker assigned to
                        // this proof, but now worker_registry is returning None, meaning the
                        // worker that was assign stopped responded and was removed from the
                        // registry
                        return Ok(Some(ProofStatus::lost()));
                    } else {
                        // no worker is assigned to this proof
                        debug!("proof {request_id} was not assigned to a worker");
                        // otherwise, the registry doesn't have any worker assigned to this proof
                        return Ok(None);
                    }
                }
                // got proof status from worker
                Some(Ok(proof_status)) => {
                    return Ok(Some(proof_status));
                }
                // Worker assigned to this proof didn't return proof status
                Some(Err(worker_addr)) => {
                    error!(
                        "Worker {worker_addr} assigned to proof {request_id} didn't return proof status"
                    );
                    // we know a worker was working on this proof, but we didn't get a response
                    // from them
                    a_worker_is_assigned = true;
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
}

pub struct WorkerRegistry {
    pub cfg_max_worker_strikes: usize,
    pub cfg_max_worker_heartbeat_interval_secs: Duration,
    // Using a HashMap is a fine complexity tradeoff because we'll never have >20 workers, so
    // iterating isn't horrible in reality.
    pub workers: HashMap<String, WorkerState>,
    pub reqwest_client: Client,
    pub receiver: mpsc::Receiver<WorkerRegistryCommand>,
    // mapping proof_request_id -> ContemplantProofStatus for currently outbound proof status
    // requests that are being awaited in a thread
    pub awaiting_proof_status_responses: HashMap<B256, mpsc::Sender<ContemplantProofStatus>>,
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
                } => {
                    self.handle_worker_ready(worker_addr, worker_name).await;
                }
                WorkerRegistryCommand::ProofComplete { request_id } => {
                    self.handle_proof_complete(request_id).await;
                }
                WorkerRegistryCommand::ProofStatusResponse {
                    request_id,
                    maybe_proof_status,
                } => {
                    self.handle_proof_status_response(request_id, maybe_proof_status)
                        .await;
                }
                WorkerRegistryCommand::ProofStatus {
                    target_request_id,
                    resp_sender,
                } => {
                    self.handle_proof_status(target_request_id, resp_sender)
                        .await;
                }
                WorkerRegistryCommand::Workers { resp_sender } => {
                    self.handle_workers(resp_sender);
                }
                WorkerRegistryCommand::Heartbeat {
                    worker_addr,
                    should_drop_sender,
                } => {
                    self.handle_heartbeat(worker_addr, should_drop_sender);
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

    // iterate through workers and remove any who have > MAX_STRIKES strikes
    fn trim_workers(&mut self) {
        let dead_workers: Vec<String> = self
            .workers
            .iter_mut()
            .filter_map(|(worker_addr, worker_state)| {
                if worker_state.should_drop(
                    self.cfg_max_worker_strikes,
                    self.cfg_max_worker_heartbeat_interval_secs,
                ) {
                    Some(worker_addr.clone())
                } else {
                    None
                }
            })
            .collect();

        for dead_worker_addr in dead_workers {
            // remove them from the mapping
            if let Some(dead_worker_state) = self.workers.remove(&dead_worker_addr) {
                info!("Removing worker {dead_worker_addr} from worker registry");
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
        should_drop_sender.send(should_drop);
    }

    async fn handle_assign_proof(&mut self, proof_request: &ContemplantProofRequest) {
        // remove any dead workers
        self.trim_workers();
        let request_id = proof_request.request_id;

        // first check if there's already a worker working on this proof
        if let Some((worker_addr, _)) = self.workers.iter().find(|(_, worker_state)| {
            if let WorkerStatus::Busy {
                request_id: workers_request_id,
                ..
            } = worker_state.status
            {
                workers_request_id == request_id
            } else {
                false
            }
        }) {
            info!(
                "Received proof request for {} proof {} but worker {} is already busy with it",
                proof_request.mode.as_str_name(),
                request_id,
                worker_addr
            );
            // there's already a worker proving this.  We can return
            // early
            return;
        }

        // iterate over all idle workers
        for (worker_addr, worker_state) in self.workers.iter_mut() {
            debug!("Worker {} state {}", worker_addr, worker_state);

            // if this worker isn't idle, skip
            if worker_state.is_busy() {
                continue;
            }

            info!("Attemping to assign proof request {request_id} to worker {worker_addr}");

            // TODO: this blocks up things for AWHILE
            let worker_response = self
                .reqwest_client
                .post(format!("{}/request_proof", worker_addr))
                .json(&proof_request)
                .send()
                .await;

            let response = match worker_response {
                Ok(response) => response,
                Err(err) => {
                    // TODO: could make a StrikeWorker command then make handling
                    // reqwest responses more async by moving them to a tokio task
                    worker_state.add_strike();
                    error!(
                        "Failed to send request for proof {} to worker {}. Error: {}",
                        request_id, worker_addr, err
                    );
                    // go to next loop iteration
                    continue;
                }
            };

            if response.status().is_success() {
                info!(
                    "Successfully assigned proof {} to worker {}",
                    request_id, worker_addr
                );

                // successfully assigned proof, can exit
                worker_state.assigned_proof(request_id, proof_request.mode);
                return;
            } else {
                // TODO: could make a StrikeWorker command then make handling
                // reqwest responses more async by moving them to a tokio task
                worker_state.add_strike();
                error!(
                    "Failed to assign proof {} to worker {}. Status code {}: {:?}",
                    request_id,
                    worker_addr,
                    response.status().as_u16(),
                    response.status().canonical_reason()
                );
            }
        }
        // We iterated through all the workers and couldn't find an idle one who could
        // receive the request.
        //
        // This doesn't result in deadlock because the proposer will call `/status/request_id`
        // which will trigger another AssignProof request here

        warn!("No workers available for proof {request_id}");
    }

    async fn handle_worker_ready(&mut self, worker_addr: String, worker_name: String) {
        let default_state = WorkerState::new(worker_name.clone());
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
                ..
            } = worker_state.status
            {
                // if they're marked as busy with the proof we just saw completed
                if busy_request_id == request_id {
                    // We know the worker is done with this proof because it just
                    // returned us an executed proof.

                    // move worker from "busy" to "idle"
                    debug!("Worker {} completed a proof and is now Idle.", worker_addr);
                    worker_state.status = WorkerStatus::Idle;
                }
            }
        } else {
            error!("Worker registry couldn't find worker who was assigned proof {request_id}");
        }
    }

    // forwards proof status to the thread awaiting it
    async fn handle_proof_status_response(
        &mut self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) {
        // remove the sender from this mapping and send the proof status to the thread awaiting it
        let sender = match self.awaiting_proof_status_responses.remove(&request_id) {
            Some(s) => s,
            None => {
                return;
            }
        };

        if let Err(e) = sender.send(maybe_proof_status).await {
            error!("{e}");
        }
    }

    // TODO: spawn a task that mimics http request/response that waits for the status response from
    // the worker
    async fn handle_proof_status(
        &mut self,
        target_request_id: B256,
        resp_sender: oneshot::Sender<Option<std::result::Result<ProofStatus, String>>>,
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
                    resp_sender.send(None).unwrap();
                    return;
                }
            };

        info!("Worker {} is {}", worker_state.name, worker_state.status);

        // forward proof_status request to worker
        let worker_proof_status_request = || {
            self.reqwest_client
                .get(format!("{}/status/{}", worker_addr, target_request_id))
                .send()
        };

        let retries = 3;
        let response = match request_with_retries(retries, worker_proof_status_request).await {
            Ok(worker_response) => worker_response,
            Err(err) => {
                // TODO: could make a StrikeWorker command then make handling
                // reqwest responses more async by moving them to a tokio task
                worker_state.add_strikes(retries);
                error!(
                    "Failed to send request {}/status/{}. Error: {}",
                    worker_addr, target_request_id, err
                );
                // there's a worker assigned but we can't communicate with it.  Assume
                // it's dead & tell coordinator we lost the proof
                resp_sender.send(Some(Ok(ProofStatus::lost()))).unwrap();

                // TODO: reassign proof

                return;
            }
        };

        if response.status().is_success() {
            let contemplant_proof_status: ContemplantProofStatus = match response.json().await {
                Ok(proof_status) => proof_status,
                Err(err) => {
                    worker_state.add_strike();
                    error!(
                        "Error deserializing response from {}/status/{}.Error: {}",
                        worker_addr, target_request_id, err
                    );

                    // can't deserialize request
                    resp_sender.send(Some(Err(worker_addr.clone()))).unwrap();

                    return;
                }
            };

            // TODO: implement From<ContemplantProofStatus> for ProofStatus
            let proof_status = match contemplant_proof_status.proof {
                Some(proof) => ProofStatus {
                    fulfillment_status: FulfillmentStatus::Fulfilled.into(),
                    execution_status: ExecutionStatus::Executed.into(),
                    proof,
                },
                None => ProofStatus {
                    fulfillment_status: FulfillmentStatus::Assigned.into(),
                    execution_status: ExecutionStatus::Unexecuted.into(),
                    proof: Vec::new(),
                },
            };

            resp_sender.send(Some(Ok(proof_status))).unwrap();
        } else {
            // TODO: could make a StrikeWorker command then make handling
            // reqwest responses more async by moving them to a tokio task
            worker_state.add_strike();
            error!(
                "Failed to get response from {}/status/{}. Status code {}: {:?}",
                worker_addr,
                target_request_id,
                response.status().as_u16(),
                response.status().canonical_reason()
            );

            // response status not-ok
            resp_sender.send(Some(Err(worker_addr.clone()))).unwrap();
            // TODO: reassign proof
        }
    }

    fn handle_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let workers = self
            .workers
            .iter()
            .map(|(x, y)| (x.clone(), y.clone()))
            .collect();
        resp_sender.send(workers).unwrap();
    }
}

pub enum WorkerRegistryCommand {
    AssignProofRequest {
        proof_request: ContemplantProofRequest,
    },
    WorkerReady {
        worker_addr: String,
        worker_name: String,
    },
    // triggered when sp1_sdk requests proof status.
    // sends proof status request to contemplant
    // Spawns a thread that awaits a ContemplantProofStatusRequest
    ProofStatusDispatcher {
        target_request_id: B256,
    },
    // contemplant responding with a proof status that was previously requested
    ProofStatusResponse {
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofRequest>,
    },
    // responds to sp1_sdk proof status request
    ProofStatusResponder {
        resp_sender: oneshot::Sender<Option<std::result::Result<ProofStatus, String>>>,
    },
    ProofComplete {
        request_id: B256,
    },
    Workers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
    Heartbeat {
        worker_addr: String,
        should_drop_sender: oneshot::Sender<bool>,
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
            WorkerRegistryCommand::ProofStatus { .. } => {
                format!("ProofStatus")
            }
            WorkerRegistryCommand::Workers { .. } => {
                format!("Workers")
            }
        };
        write!(f, "{command}")
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct WorkerState {
    name: String,
    status: WorkerStatus,
    strikes: usize,
    last_heartbeat: Instant,
}

impl WorkerState {
    fn new(name: String) -> Self {
        Self {
            name,
            status: WorkerStatus::Idle,
            strikes: 0,
            last_heartbeat: Instant::now(),
        }
    }
    fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
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
        };
        // This worker has been good.  Reset their strikes
        self.strikes = 0;
    }

    fn heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    // drop worker if they have too many strikes OR
    // if it's been too long since their last heartbeat
    fn should_drop(
        &self,
        cfg_max_worker_strikes: usize,
        cfg_max_worker_heartbeat_interval_secs: Duration,
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

#[derive(Eq, PartialEq, Debug, Clone)]
pub enum WorkerStatus {
    Idle,
    Busy {
        request_id: B256,
        proof_mode: ProofMode,
        start_time: Instant,
    },
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy {
                request_id,
                proof_mode,
                start_time,
            } => {
                let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;
                write!(
                    f,
                    "busy with {} proof {request_id} for {minutes} minutes",
                    proof_mode.as_str_name()
                )
            }
        }
    }
}
