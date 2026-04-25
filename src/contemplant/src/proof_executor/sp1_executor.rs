use super::assessor::start_assessor;
use crate::worker_state::{ActiveSp1Prover, WorkerState};

use anyhow::{Context, anyhow};
use log::{error, info};
use network_lib::{ContemplantProofStatus, Sp1ProofRequest, to_proof_from_network};
use sp1_sdk::proof::ProofFromNetwork;
use sp1_sdk::{
    CpuProver, Prover,
    network::proto::network::{ExecutionStatus, ProofMode},
};
use std::sync::Arc;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

#[derive(Clone)]
pub struct Sp1Executor {
    pub active_prover: ActiveSp1Prover,
    pub mock_prover: Arc<CpuProver>,
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

        let (pk, _) = if mock {
            executor.mock_prover.setup(&proof_request.elf)
        } else {
            match &executor.active_prover {
                ActiveSp1Prover::Cpu(cpu_prover) => cpu_prover.setup(&proof_request.elf),
                ActiveSp1Prover::Cuda(cuda_prover) => {
                    // the cuda prover keeps state of the last `setup()` that was called on it.
                    // You must call `setup()` then `prove` *each* time you intend to
                    // prove a certain program
                    cuda_prover.setup(&proof_request.elf)
                }
            }
        };

        // construct proving function based on ProofMode and prover type
        let proof_res = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => Err(anyhow!("UnspecifiedProofMode")),
            ProofMode::Core => {
                if mock {
                    executor.mock_prover.prove(&pk, stdin).core().run()
                } else {
                    match &executor.active_prover {
                        ActiveSp1Prover::Cpu(p) => p.prove(&pk, stdin).core().run(),
                        ActiveSp1Prover::Cuda(p) => p.prove(&pk, stdin).core().run(),
                    }
                }
            }
            ProofMode::Compressed => {
                if mock {
                    executor.mock_prover.prove(&pk, stdin).compressed().run()
                } else {
                    match &executor.active_prover {
                        ActiveSp1Prover::Cpu(p) => p.prove(&pk, stdin).compressed().run(),
                        ActiveSp1Prover::Cuda(p) => p.prove(&pk, stdin).compressed().run(),
                    }
                }
            }
            ProofMode::Plonk => {
                if mock {
                    executor.mock_prover.prove(&pk, stdin).plonk().run()
                } else {
                    match &executor.active_prover {
                        ActiveSp1Prover::Cpu(p) => p.prove(&pk, stdin).plonk().run(),
                        ActiveSp1Prover::Cuda(p) => p.prove(&pk, stdin).plonk().run(),
                    }
                }
            }
            ProofMode::Groth16 => {
                if mock {
                    executor.mock_prover.prove(&pk, stdin).groth16().run()
                } else {
                    match &executor.active_prover {
                        ActiveSp1Prover::Cpu(p) => p.prove(&pk, stdin).groth16().run(),
                        ActiveSp1Prover::Cuda(p) => p.prove(&pk, stdin).groth16().run(),
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
