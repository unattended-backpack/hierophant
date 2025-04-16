use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use log::{debug, error, info, trace, warn};
use reqwest::Client;
use serde::Serialize;
use std::{
    collections::HashMap,
    fmt::{self, Display},
};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, sleep},
};

#[derive(Clone, Debug)]
pub struct WorkerRegistryClient {
    pub sender: mpsc::Sender<WorkerRegistryCommand>,
}

impl Default for WorkerRegistryClient {
    fn default() -> Self {
        Self::new(3)
    }
}

impl WorkerRegistryClient {
    pub fn new(cfg_max_worker_strikes: usize) -> Self {
        let workers = HashMap::new();
        let reqwest_client = Client::new();

        let (sender, receiver) = mpsc::channel(100);

        let worker_registry = WorkerRegistry {
            cfg_max_worker_strikes,
            workers,
            reqwest_client,
            receiver,
            self_command_sender: sender.clone(),
        };

        tokio::task::spawn(async move { worker_registry.background_event_loop().await });

        Self { sender }
    }

    pub async fn worker_ready(&self, worker_addr: String) -> Result<()> {
        self.sender
            .send(WorkerRegistryCommand::WorkerReady { worker_addr })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command WorkerReady: {}", e))
    }

    pub async fn assign_proof_request(
        &self,
        proof_id: B256,
        proof_request: GenericProofRequest,
        mock_mode: bool,
    ) -> Result<()> {
        self.sender
            .send(WorkerRegistryCommand::AssignProofRequest {
                proof_id,
                proof_request,
                mock_mode,
            })
            .await
            .map_err(|e| anyhow::anyhow!("Failed to send command AssignProofRequest: {}", e))
    }

    // run until we get None (no worker has this proof) or Some(Ok(ProofStatus)).
    // Each run it potentially trims naughty workers
    pub async fn proof_status(&self, proof_id: B256) -> Result<Option<ProofStatus>> {
        let mut a_worker_is_assigned = false;
        loop {
            let (resp_sender, receiver) = oneshot::channel();
            self.sender
                .send(WorkerRegistryCommand::ProofStatus {
                    target_proof_id: proof_id,
                    resp_sender,
                })
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send command ProofStatus: {}", e))?;

            match receiver.await? {
                // worker_registry doesn't have any worker assigned to this proof
                None => {
                    if a_worker_is_assigned {
                        error!(
                            "Worker assigned to proof {proof_id} was kicked from the worker registry."
                        );
                        // we know from a previous response that there was a worker assigned to
                        // this proof, but now worker_registry is returning None, meaning the
                        // worker that was assign stopped responded and was removed from the
                        // registry
                        return Ok(Some(ProofStatus::lost()));
                    } else {
                        // no worker is assigned to this proof
                        debug!("proof {proof_id} was not assigned to a worker");
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
                        "Worker {worker_addr} assigned to proof {proof_id} didn't return proof status"
                    );
                    // we know a worker was working on this proof, but we didn't get a response
                    // from them
                    a_worker_is_assigned = true;
                }
            }
        }
    }

    // signal that the proposer got the proof and the worker is ready to receive a new proof
    pub async fn proof_complete(&self, proof_id: B256) -> Result<()> {
        self.sender
            .send(WorkerRegistryCommand::ProofComplete { proof_id })
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
    // Using a HashMap is a fine complexity tradeoff because we'll never have >20 workers, so
    // iterating isn't horrible in reality.
    pub workers: HashMap<String, WorkerState>,
    pub reqwest_client: Client,
    pub receiver: mpsc::Receiver<WorkerRegistryCommand>,
    // for sending the task to the back of the channel queue to allow for other events to
    // process
    pub self_command_sender: mpsc::Sender<WorkerRegistryCommand>,
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
                WorkerRegistryCommand::AssignProofRequest {
                    proof_id,
                    ref proof_request,
                    mock_mode,
                } => {
                    self.handle_assign_proof(mock_mode, proof_id, proof_request)
                        .await;
                }
                WorkerRegistryCommand::WorkerReady { worker_addr } => {
                    self.handle_worker_ready(worker_addr).await;
                }
                WorkerRegistryCommand::ProofComplete { proof_id } => {
                    self.handle_proof_complete(proof_id).await;
                }
                WorkerRegistryCommand::ProofStatus {
                    target_proof_id,
                    resp_sender,
                } => {
                    self.handle_proof_status(target_proof_id, resp_sender).await;
                }
                WorkerRegistryCommand::Workers { resp_sender } => {
                    self.handle_workers(resp_sender);
                }
            };

            info!(
                "Took {} seconds to process worker_registry command {:?}",
                start.elapsed().as_secs_f64(),
                command_string
            );
        }
    }

    // iterate through workers and remove any who have > MAX_STRIKES strikes
    fn trim_workers(&mut self) {
        let dead_workers: Vec<String> = self
            .workers
            .iter_mut()
            .filter_map(|(worker_addr, worker_state)| {
                if worker_state.should_drop(self.cfg_max_worker_strikes) {
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
                if let Some(dangling_proof) = dead_worker_state.current_proof_id() {
                    // The dangling proof will eventually be requested for by the proposer via
                    // `proof_status` and the registry will return None, which will cause the
                    // coordinator to return `ProofStatus::lost()`, which will cause the proposer
                    // to re-request the proof

                    warn!(
                        "Proof {dangling_proof} left incomplete as a result of killing worker {dead_worker_addr}"
                    );
                }
            }
        }
    }

    async fn handle_assign_proof(
        &mut self,
        mock_mode: bool,
        proof_id: B256,
        proof_request: &GenericProofRequest,
    ) {
        // remove any dead workers
        self.trim_workers();

        // first check if there's already a worker working on this proof
        if let Some((worker_addr, _)) = self.workers.iter().find(|(_, worker_state)| {
            if let WorkerStatus::Busy {
                proof_id: workers_proof_id,
            } = worker_state.status
            {
                workers_proof_id == proof_id
            } else {
                false
            }
        }) {
            info!(
                "Received proof request for proof {} but worker {} is already busy with it",
                proof_id, worker_addr
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

            info!("Attemping to assign proof {proof_id} to worker {worker_addr}");
            // TODO: this blocks up things for AWHILE (entire time witnessgen is going on)
            let worker_response = match &proof_request {
                GenericProofRequest::Agg(agg_proof_request) => {
                    let worker_agg_proof_request = WorkerAggProofRequest {
                        mock_mode,
                        proof_id,
                        // TODO: this is an expensive clone
                        subproofs: agg_proof_request.subproofs.clone(),
                        head: agg_proof_request.head.clone(),
                    };
                    self.reqwest_client
                        .post(format!("{}/request_agg_proof", worker_addr))
                        .json(&worker_agg_proof_request)
                        .send()
                        .await
                }
                GenericProofRequest::Span(span_proof_request) => {
                    let worker_span_proof_request = WorkerSpanProofRequest {
                        mock_mode,
                        proof_id,
                        start: span_proof_request.start,
                        end: span_proof_request.end,
                    };
                    self.reqwest_client
                        .post(format!("{}/request_span_proof", worker_addr))
                        .json(&worker_span_proof_request)
                        .send()
                        .await
                }
            };

            let response = match worker_response {
                Ok(response) => response,
                Err(err) => {
                    // TODO: could make a StrikeWorker command then make handling
                    // reqwest responses more async by moving them to a tokio task
                    worker_state.add_strike();
                    error!(
                        "Failed to send request for proof {} to worker {}. Error: {}",
                        proof_id, worker_addr, err
                    );
                    // go to next loop iteration
                    continue;
                }
            };

            if response.status().is_success() {
                info!(
                    "Successfully assigned proof {} to worker {}",
                    proof_id, worker_addr
                );

                // successfully assigned proof, can exit
                worker_state.assigned_proof(proof_id);
                return;
            } else {
                // TODO: could make a StrikeWorker command then make handling
                // reqwest responses more async by moving them to a tokio task
                worker_state.add_strike();
                error!(
                    "Failed to assign proof {} to worker {}. Status code {}: {:?}",
                    proof_id,
                    worker_addr,
                    response.status().as_u16(),
                    response.status().canonical_reason()
                );
            }
        }
        // We iterated through all the workers and couldn't find an idle one who could
        // receive the request.
        //
        // This doesn't result in deadlock because the proposer will call `/status/proof_id`
        // which will trigger another AssignProof request here

        warn!("No workers available for proof {proof_id}");
    }

    async fn handle_worker_ready(&mut self, worker_addr: String) {
        let default_state = WorkerState::default();
        match self
            .workers
            .insert(worker_addr.clone(), default_state.clone())
        {
            Some(old_state) => {
                // if this worker was working on a proof but we didn't drop it
                if old_state.is_busy() && !old_state.should_drop(self.cfg_max_worker_strikes) {
                    error!(
                        "Worker {} re-started but wasn't dropped yet.  Worker State: {}",
                        worker_addr, old_state
                    );
                } else {
                    info!(
                        "Known worker {} re-started, resetting state from {} to {}",
                        worker_addr, old_state, default_state
                    );
                }
            }
            None => {
                info!("New worker {} added to registry", worker_addr);
            }
        }
    }

    async fn handle_proof_complete(&mut self, proof_id: B256) {
        if let Some((worker_addr, worker_state)) = self
            .workers
            .iter_mut()
            .find(|(_, worker_state)| worker_state.current_proof_id() == Some(proof_id))
        {
            // move worker from "busy" to "idle"
            debug!("Worker {} completed a proof and is now Idle.", worker_addr);
            worker_state.status = WorkerStatus::Idle;
        } else {
            error!("Worker registry couldn't find worker who was assigned proof {proof_id}");
        }
    }

    async fn handle_proof_status(
        &mut self,
        target_proof_id: B256,
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
                    WorkerStatus::Busy { proof_id } => proof_id == target_proof_id,
                }) {
                Some(worker_assigned) => worker_assigned,
                None => {
                    // This proof wasn't assigned to any worker, return none
                    info!("No worker is assigned to proof {}", target_proof_id);
                    resp_sender.send(None);
                    return;
                }
            };

        // forward proof_status request to worker
        let worker_proof_status_request = || {
            self.reqwest_client
                .get(format!("{}/status/{}", worker_addr, target_proof_id))
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
                    worker_addr, target_proof_id, err
                );
                // there's a worker assigned but we can't communicate with it.  Assume
                // it's dead & tell coordinator we lost the proof
                resp_sender.send(Some(Ok(ProofStatus::lost())));

                return;
            }
        };

        if response.status().is_success() {
            let proof_status: ProofStatus = match response.json().await {
                Ok(proof_status) => proof_status,
                Err(err) => {
                    worker_state.add_strike();
                    error!(
                        "Error deserializing response from {}/status{}.Error: {}",
                        worker_addr, target_proof_id, err
                    );

                    // can't deserialize request
                    resp_sender.send(Some(Err(worker_addr.clone())));

                    return;
                }
            };
            debug!(
                "ProofStatus of {} from worker {}: {}",
                target_proof_id, worker_addr, proof_status
            );

            resp_sender.send(Some(Ok(proof_status)));
        } else {
            // TODO: could make a StrikeWorker command then make handling
            // reqwest responses more async by moving them to a tokio task
            worker_state.add_strike();
            error!(
                "Failed to get response from {}/status/{}. Status code {}: {:?}",
                worker_addr,
                target_proof_id,
                response.status().as_u16(),
                response.status().canonical_reason()
            );

            // response status not-ok
            resp_sender.send(Some(Err(worker_addr.clone())));
        }
    }

    fn handle_workers(&self, resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>) {
        let workers = self
            .workers
            .iter()
            .map(|(x, y)| (x.clone(), y.clone()))
            .collect();
        resp_sender.send(workers);
    }
}

pub enum WorkerRegistryCommand {
    AssignProofRequest {
        proof_id: B256,
        proof_request: GenericProofRequest,
        mock_mode: bool,
    },
    WorkerReady {
        worker_addr: String,
    },
    ProofStatus {
        target_proof_id: B256,
        // returns the proof_status to the calling thread
        // returns None if there is no worker assigned to this proof
        // returns Some(Err(worker_addr)) if there is worker assigned to that is communicating with
        // us but we can't get the proof status out of it
        resp_sender: oneshot::Sender<Option<std::result::Result<ProofStatus, String>>>,
    },
    ProofComplete {
        proof_id: B256,
    },
    Workers {
        resp_sender: oneshot::Sender<Vec<(String, WorkerState)>>,
    },
}

impl fmt::Debug for WorkerRegistryCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let command = match self {
            WorkerRegistryCommand::AssignProofRequest {
                proof_id,
                ref proof_request,
                mock_mode,
            } => {
                format!("AssignProofRequest")
            }
            WorkerRegistryCommand::WorkerReady { worker_addr } => {
                format!("WorkerReady")
            }
            WorkerRegistryCommand::ProofComplete { proof_id } => {
                format!("ProofComplete")
            }
            WorkerRegistryCommand::ProofStatus {
                target_proof_id,
                resp_sender,
            } => {
                format!("ProofStatus")
            }
            WorkerRegistryCommand::Workers { resp_sender } => {
                format!("Workers")
            }
        };
        write!(f, "{command}")
    }
}

#[derive(Eq, PartialEq, Clone, Debug, Serialize)]
pub struct WorkerState {
    status: WorkerStatus,
    strikes: usize,
}

impl Default for WorkerState {
    fn default() -> Self {
        Self {
            status: WorkerStatus::Idle,
            strikes: 0,
        }
    }
}

impl WorkerState {
    fn is_busy(&self) -> bool {
        self.status != WorkerStatus::Idle
    }

    fn add_strike(&mut self) {
        self.strikes += 1;
        debug!("Strike added to worker.  New strikes: {}", self.strikes);
    }

    fn add_strikes(&mut self, strikes: usize) {
        self.strikes += strikes;
        debug!(
            "{} strikes added to worker.  New strikes: {}",
            strikes, self.strikes
        );
    }

    fn assigned_proof(&mut self, proof_id: B256) {
        self.status = WorkerStatus::Busy { proof_id };
        // This worker has been good.  Reset their strikes
        self.strikes = 0;
    }

    fn should_drop(&self, cfg_max_worker_strikes: usize) -> bool {
        self.strikes >= cfg_max_worker_strikes
    }

    // returns the proof it's currently working on, if any
    fn current_proof_id(&self) -> Option<B256> {
        match self.status {
            WorkerStatus::Idle => None,
            WorkerStatus::Busy { proof_id } => Some(proof_id),
        }
    }
}

impl Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Worker status: {}, Worker strikes: {}",
            self.status, self.strikes
        )
    }
}

#[derive(Eq, PartialEq, Debug, Clone, Serialize)]
pub enum WorkerStatus {
    Idle,
    Busy { proof_id: B256 },
}

impl Display for WorkerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::Busy { proof_id } => write!(f, "Busy with proof {proof_id}"),
        }
    }
}
