use super::assessor::start_assessor;
use crate::worker_state::{ActiveSp1Prover, WorkerState};

use anyhow::{Context, anyhow};
use log::{error, info};
use network_lib::{ContemplantProofStatus, Sp1ProofRequest, to_proof_from_network};
use sp1_sdk::proof::ProofFromNetwork;
use sp1_sdk::{
    MockProver, ProveRequest, Prover, SP1ProofMode, SP1ProofWithPublicValues,
    network::proto::base::types::{ExecutionStatus, ProofMode},
};
use std::sync::Arc;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct Sp1Executor {
    pub active_prover: ActiveSp1Prover,
    pub mock_prover: Arc<MockProver>,
    pub progress_tracking_available: bool,
}

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode,
// and SP1Stdin provided by the Hierophant
pub(super) async fn execute(
    state: WorkerState,
    executor: Sp1Executor,
    proof_request: Sp1ProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
    info!(
        "Received SP1 proof request {} (mode {})",
        proof_request.request_id,
        proof_request.mode.as_str_name()
    );

    let initial_status = ContemplantProofStatus::unexecuted();

    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    let (assessor_shutdown_tx, assessor_shutdown_rx) = watch::channel(false);

    let mock_prover_clone = executor.mock_prover.clone();
    let elf_clone = proof_request.elf.clone();
    let stdin_clone = proof_request.sp1_stdin.clone();
    let proof_store_client_clone = state.proof_store_client.clone();
    let progress_tracking_available = executor.progress_tracking_available;
    let assessor_config = state.assessor_config.clone();
    let request_id = proof_request.request_id;
    let display = format!(
        "SP1 {} proof with request id {}",
        proof_request.mode.as_str_name(),
        request_id
    );
    tokio::task::spawn(async move {
        if let Err(e) = start_assessor(
            mock_prover_clone,
            &elf_clone,
            &stdin_clone,
            assessor_config,
            assessor_shutdown_rx,
            proof_store_client_clone,
            request_id,
            progress_tracking_available,
        )
        .await
        {
            error!("Assessor error: {e}");
        }

        let start_time = Instant::now();
        let mock = proof_request.mock;
        let stdin = &proof_request.sp1_stdin;

        // Map the request's wire-level mode onto the SDK's mode once; 6.x
        // prove requests take the mode as a builder argument, replacing the
        // per-mode method chains of 5.x.
        let sp1_mode = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => None,
            ProofMode::Core => Some(SP1ProofMode::Core),
            ProofMode::Compressed => Some(SP1ProofMode::Compressed),
            ProofMode::Plonk => Some(SP1ProofMode::Plonk),
            ProofMode::Groth16 => Some(SP1ProofMode::Groth16),
        };

        // setup() + prove() must stay within one prover branch: each 6.x
        // prover has its own proving-key type (the cuda prover's key is a
        // remote handle to state held by the sp1-gpu-server, re-created by
        // each setup() call), so the pair can't be hoisted out and shared
        // across branches the way the 5.x code did.
        let elf = proof_request.elf.clone();
        let proof_res: anyhow::Result<SP1ProofWithPublicValues> = match sp1_mode {
            None => Err(anyhow!("UnspecifiedProofMode")),
            Some(sp1_mode) => {
                if mock {
                    async {
                        let pk = executor.mock_prover.setup(elf.into()).await?;
                        executor
                            .mock_prover
                            .prove(&pk, stdin.clone())
                            .mode(sp1_mode)
                            .await
                    }
                    .await
                } else {
                    match &executor.active_prover {
                        ActiveSp1Prover::Cpu(p) => {
                            async {
                                let pk = p.setup(elf.into()).await?;
                                Ok(p.prove(&pk, stdin.clone()).mode(sp1_mode).await?)
                            }
                            .await
                        }
                        ActiveSp1Prover::Cuda(p) => {
                            async {
                                let pk = p.setup(elf.into()).await?;
                                Ok(p.prove(&pk, stdin.clone()).mode(sp1_mode).await?)
                            }
                            .await
                        }
                    }
                }
            }
        };

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        let proof_bytes_res = proof_res.and_then(|proof| {
            let network_proof: ProofFromNetwork = to_proof_from_network(proof);
            bincode::serialize(&network_proof).map_err(|e| anyhow!("Error serializing proof {e}"))
        });

        match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed {display} in {minutes} minutes");
                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
                    )
                    .await;
            }
            Err(e) => {
                let error_msg = format!("Error proving {display} at minute {minutes}: {e}");

                // If a contemplant errors while making a proof it should seppuku.
                exit_sender
                    .send(error_msg)
                    .await
                    .context("Send exit error message to main thread")
                    .unwrap();

                state
                    .proof_store_client
                    .proof_status_update(
                        request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;
            }
        };

        if let Err(err) = assessor_shutdown_tx.send(true) {
            error!("Error sending shutdown signal to assessor: {err}");
        }
    });
}
