use super::command::WorkerRegistryCommand;
use super::registry::WorkerRegistry;
use super::worker_state::WorkerState;
use crate::config::WorkerRegistryConfig;
use crate::proof::{CompletedProofInfo, ProofStatus};

use alloy_primitives::B256;
use anyhow::{Result, anyhow};
use log::{error, warn};
use network_lib::{
    ContemplantProofRequest, ContemplantProofStatus, WorkerRegisterInfo,
    messages::FromHierophantMessage, protocol::CONTEMPLANT_VERSION,
};
use std::collections::HashMap;
use std::ops::ControlFlow;
use tokio::{
    sync::{mpsc, oneshot},
    time::{Duration, timeout},
};

#[derive(Clone, Debug)]
pub struct WorkerRegistryClient {
    pub sender: mpsc::Sender<WorkerRegistryCommand>,
}

impl WorkerRegistryClient {
    pub fn new(config: WorkerRegistryConfig) -> Self {
        let workers = HashMap::new();
        let dead_workers = Vec::new();

        let (sender, receiver) = mpsc::channel(100);

        let awaiting_proof_status_responses = HashMap::new();

        let proof_history = Vec::new();

        let reqwest_client = reqwest::Client::new();

        let worker_registry = WorkerRegistry {
            config,
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
            error!("Failed to send command Heartbeat: {e}");
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
                magister_drop_endpoint: worker_register_info.magister_drop_endpoint,
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
