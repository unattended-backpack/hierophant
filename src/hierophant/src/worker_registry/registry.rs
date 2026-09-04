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
use sp1_sdk::network::proto::base::types::ExecutionStatus;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime};
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
    // Requests that arrived while no idle capable worker existed, oldest
    // first, each stamped with its arrival time. Drained whenever
    // capacity appears (worker registers, completes, or fails a proof).
    // Without this queue such requests were silently dropped and the
    // client's first status poll turned them into terminal failures.
    pub pending_requests: VecDeque<(ContemplantProofRequest, Instant)>,
}

/// Most requests a full fleet outage is allowed to buffer before new
/// arrivals are rejected (the client then sees the old lost-request
/// behavior). Sized far above any sane proposer fan-out; the point is
/// bounding memory, since a queued `wrap_of` request can carry a
/// multi-MB receipt.
const PENDING_QUEUE_CAP: usize = 64;

/// How long a request may wait in the pending queue before it is
/// dropped. Clients enforce their own (shorter) proof deadlines; this
/// is just the backstop that keeps abandoned requests from pinning
/// their payloads forever.
const PENDING_TTL: Duration = Duration::from_secs(2 * 60 * 60);


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
                    openvm_evm_enabled,
                    magister_drop_endpoint,
                    instance_nonce,
                    from_hierophant_sender,
                } => {
                    self.handle_worker_ready(
                        worker_addr,
                        worker_name,
                        supported_vms,
                        groth16_enabled,
                        openvm_evm_enabled,
                        magister_drop_endpoint,
                        instance_nonce,
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
                WorkerRegistryCommand::PendingInfo { resp_sender } => {
                    let oldest_secs = self
                        .pending_requests
                        .front()
                        .map(|(_, queued_at)| queued_at.elapsed().as_secs());
                    let _ = resp_sender.send((self.pending_requests.len(), oldest_secs));
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
        // remove any dead workers
        self.trim_workers();

        if self.try_assign(proof_request).await {
            return;
        }

        // No idle capable worker right now: queue the request so the
        // next capacity event (worker registers, completes, or fails a
        // proof) picks it up, instead of dropping it — a dropped
        // request turns into a terminal failure on the client's first
        // status poll.
        let request_id = proof_request.request_id();
        if self
            .pending_requests
            .iter()
            .any(|(r, _)| r.request_id() == request_id)
        {
            return;
        }
        if self.pending_requests.len() >= PENDING_QUEUE_CAP {
            warn!(
                "Pending queue full ({PENDING_QUEUE_CAP}); dropping proof request {request_id}"
            );
            return;
        }
        info!(
            "No idle capable worker for {} {} proof {request_id}; queued (pending: {})",
            proof_request.vm(),
            proof_request.mode_name(),
            self.pending_requests.len() + 1
        );
        self.pending_requests
            .push_back((proof_request.clone(), Instant::now()));
    }

    /// One assignment attempt. Returns true when the request found a
    /// home (or a worker is already busy with it); false leaves the
    /// caller to queue or drop it.
    async fn try_assign(&mut self, proof_request: &ContemplantProofRequest) -> bool {
        let request_id = proof_request.request_id();
        let target_vm = proof_request.vm();
        let mode_name = proof_request.mode_name();
        let needs_groth16 = proof_request.needs_groth16();
        let needs_openvm_evm = proof_request.needs_openvm_evm();

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
                    return true;
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
                    return true;
                }
            }
        }
        // We iterated through all the workers and couldn't find an idle
        // one who could receive the request.
        let requirement = if needs_groth16 {
            format!("{target_vm}-capable Groth16-enabled")
        } else if needs_openvm_evm {
            format!("{target_vm}-capable EVM-enabled")
        } else {
            format!("{target_vm}-capable")
        };
        debug!("No {requirement} idle workers available for proof {request_id}");
        false
    }

    /// Try to place queued requests onto whatever capacity just
    /// appeared. Entries older than [`PENDING_TTL`] are dropped first;
    /// entries that still find no worker keep their queue position.
    async fn drain_pending(&mut self) {
        self.pending_requests.retain(|(request, queued_at)| {
            let stale = queued_at.elapsed() > PENDING_TTL;
            if stale {
                warn!(
                    "Dropping pending proof {} after {}s in queue",
                    request.request_id(),
                    queued_at.elapsed().as_secs()
                );
            }
            !stale
        });

        let mut still_pending = VecDeque::new();
        while let Some((request, queued_at)) = self.pending_requests.pop_front() {
            if self.try_assign(&request).await {
                info!(
                    "Assigned queued proof {} after {}s in queue",
                    request.request_id(),
                    queued_at.elapsed().as_secs()
                );
            } else {
                still_pending.push_back((request, queued_at));
            }
        }
        self.pending_requests = still_pending;
    }

    async fn handle_worker_ready(
        &mut self,
        worker_addr: String,
        worker_name: String,
        supported_vms: Vec<VmKind>,
        groth16_enabled: bool,
        openvm_evm_enabled: bool,
        magister_drop_endpoint: Option<String>,
        instance_nonce: u64,
        from_hierophant_sender: mpsc::Sender<FromHierophantMessage>,
    ) {
        // A reconnecting contemplant arrives under a fresh peer addr, so
        // its previous registration lingers under the old key. Locate it
        // by instance identity — the magister drop endpoint is unique per
        // instance; the name is the fallback for magister-less workers —
        // and remove it SILENTLY, never through the eviction path: the
        // eviction's magister drop call would destroy the live instance
        // that just re-registered.
        let stale_key = self
            .workers
            .iter()
            .find(|(addr, ws)| {
                **addr != worker_addr
                    && match (&magister_drop_endpoint, &ws.magister_drop_endpoint) {
                        (Some(new_ep), Some(old_ep)) => new_ep == old_ep,
                        _ => ws.name == worker_name,
                    }
            })
            .map(|(key, _)| key.clone());

        let mut carried_status: Option<WorkerStatus> = None;
        if let Some(key) = stale_key {
            if let Some(old_state) = self.workers.remove(&key) {
                if old_state.instance_nonce == instance_nonce && old_state.is_busy() {
                    // Same process, new socket: a ws blip mid-proof. Carry
                    // the assignment over so the still-running proof keeps
                    // answering status polls and completes normally.
                    info!(
                        "Contemplant {worker_name} reconnected ({key} -> {worker_addr}); \
                         carrying its in-flight assignment over ({})",
                        old_state.status
                    );
                    carried_status = Some(old_state.status);
                } else {
                    info!(
                        "Removed stale registry entry for {worker_name} at {key} \
                         (superseded by registration from {worker_addr})"
                    );
                }
            }
        }

        let mut default_state = WorkerState::new(
            worker_name.clone(),
            supported_vms,
            groth16_enabled,
            openvm_evm_enabled,
            magister_drop_endpoint,
            instance_nonce,
            from_hierophant_sender,
        );
        if let Some(status) = carried_status {
            default_state.status = status;
        }
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

        self.drain_pending().await;
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
                    worker_state.completed_proof();
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
            self.drain_pending().await;
        } else {
            warn!("Worker registry couldn't find worker who was assigned proof {request_id}");
        }
    }

    async fn handle_proof_status_response(
        &mut self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) {
        let maybe_proof_status = self.bridge_startup_race(request_id, maybe_proof_status);

        self.free_worker_of_failed_proof(request_id, maybe_proof_status.as_ref())
            .await;

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

    // A contemplant that answers "unknown request" (None) for a proof this
    // registry JUST assigned to it is almost certainly still receiving the
    // request: the worker is marked Busy at ws-send time, but a multi-MB
    // witness takes seconds to cross the wire, and a status poll racing the
    // transfer would otherwise be relayed as a loss — which the proof
    // routes record as a STICKY terminal failure while the worker proves
    // on, orphaned. Within the grace window, substitute "unexecuted"
    // (fulfillment: Assigned) so clients keep polling; past it, relay the
    // None as a genuine loss.
    fn bridge_startup_race(
        &self,
        request_id: B256,
        maybe_proof_status: Option<ContemplantProofStatus>,
    ) -> Option<ContemplantProofStatus> {
        if maybe_proof_status.is_some() {
            return maybe_proof_status;
        }
        if let Some((worker_addr, worker_state)) =
            self.workers.iter().find(|(_, worker_state)| {
                matches!(
                    worker_state.status,
                    WorkerStatus::Busy { request_id: busy_request_id, .. }
                        if busy_request_id == request_id
                )
            })
        {
            if let WorkerStatus::Busy { start_time, .. } = &worker_state.status {
                if start_time.elapsed()
                    < Duration::from_secs(self.config.assignment_startup_grace_secs)
                {
                    info!(
                        "Worker {} at {worker_addr} does not know proof {request_id} yet \
                         ({}s after assignment); bridging as still-starting",
                        worker_state.name,
                        start_time.elapsed().as_secs()
                    );
                    return Some(ContemplantProofStatus::unexecuted());
                }
            }
        }
        None
    }

    // The failure-path counterpart to `handle_proof_complete`: a contemplant
    // reporting Unexecutable has abandoned the proof, so the worker must
    // return to Idle or it stays Busy forever and the registry deems every
    // subsequent request unfulfillable once all workers are wedged.
    async fn free_worker_of_failed_proof(
        &mut self,
        request_id: B256,
        maybe_proof_status: Option<&ContemplantProofStatus>,
    ) {
        let unexecutable = maybe_proof_status.is_some_and(|status| {
            status.execution_status == i32::from(ExecutionStatus::Unexecutable)
        });
        if !unexecutable {
            return;
        }

        if let Some((worker_addr, worker_state)) =
            self.workers.iter_mut().find(|(_, worker_state)| {
                matches!(
                    worker_state.status,
                    WorkerStatus::Busy { request_id: busy_request_id, .. }
                        if busy_request_id == request_id
                )
            })
        {
            let worker_name = worker_state.name.clone();
            worker_state.failed_proof();
            warn!(
                "Worker {worker_name} at {worker_addr} reported proof {request_id} as unexecutable and is now Idle."
            );
            self.drain_pending().await;
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
                    // A queued request is alive, just not started — report
                    // it as unexecuted rather than lost, or the client
                    // terminal-fails a proof that is simply waiting for
                    // capacity.
                    if self
                        .pending_requests
                        .iter()
                        .any(|(r, _)| r.request_id() == target_request_id)
                    {
                        info!(
                            "Proof {target_request_id} is queued awaiting an idle worker"
                        );
                        let _ =
                            resp_sender.send(Some(ContemplantProofStatus::unexecuted()));
                    } else {
                        info!("No worker is assigned to proof {target_request_id}");
                        let _ = resp_sender.send(None);
                    }
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
