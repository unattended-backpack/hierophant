use super::assessor::start_assessor;
use crate::WorkerState;

use anyhow::{Context, anyhow};
use log::{error, info};
use network_lib::{ContemplantProofRequest, ContemplantProofStatus, to_proof_from_network};
use sp1_sdk::proof::ProofFromNetwork;
use sp1_sdk::{
    Prover,
    network::proto::network::{ExecutionStatus, ProofMode},
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

// uses the CudaProver or MockProver to execute proofs given the elf, ProofMode, and SP1Stdin
// provided by the Hierophant
pub async fn execute_proof(
    state: WorkerState,
    proof_request: ContemplantProofRequest,
    exit_sender: mpsc::Sender<String>,
) {
    info!("Received proof request {proof_request}");

    // proof starts as unexecuted
    let initial_status = ContemplantProofStatus::unexecuted();

    // It is assumed that the Hierophant won't request the same proof twice
    state
        .proof_store_client
        .insert(proof_request.request_id, initial_status)
        .await;

    let (assessor_shutdown_tx, assessor_shutdown_rx) = watch::channel(false);

    let mock_prover_clone = state.mock_prover.clone();
    let elf_clone = proof_request.elf.clone();
    let stdin_clone = proof_request.sp1_stdin.clone();
    let proof_store_client_clone = state.proof_store_client.clone();
    tokio::task::spawn(async move {
        if let Err(e) = start_assessor(
            mock_prover_clone,
            &elf_clone,
            &stdin_clone,
            state.assessor_config,
            assessor_shutdown_rx,
            proof_store_client_clone,
            proof_request.request_id,
        )
        .await
        {
            error!("Assessor error: {e}");
        }

        let start_time = Instant::now();
        let mock = proof_request.mock;
        let stdin = &proof_request.sp1_stdin;

        let (pk, _) = if mock {
            state.mock_prover.setup(&proof_request.elf)
        } else {
            // the cuda prover keeps state of the last `setup()` that was called on it.
            // You must call `setup()` then `prove` *each* time you intend to
            // prove a certain program
            state.cuda_prover.setup(&proof_request.elf)
        };

        // construct proving function based on ProofMode and if it's a CUDA or mock proof
        let proof_res = match proof_request.mode {
            ProofMode::UnspecifiedProofMode => Err(anyhow!("UnspecifiedProofMode")),
            ProofMode::Core => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).core().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).core().run()
                }
            }
            ProofMode::Compressed => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).compressed().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).compressed().run()
                }
            }
            ProofMode::Plonk => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).plonk().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).plonk().run()
                }
            }
            ProofMode::Groth16 => {
                if mock {
                    state.mock_prover.prove(&pk, stdin).groth16().run()
                } else {
                    state.cuda_prover.prove(&pk, stdin).groth16().run()
                }
            }
        };

        let minutes = (start_time.elapsed().as_secs_f32() / 60.0).round() as u32;

        let proof_bytes_res = proof_res.and_then(|proof| {
            let network_proof: ProofFromNetwork = to_proof_from_network(proof);
            bincode::serialize(&network_proof).map_err(|e| anyhow!("Error serializing proof {e}"))
        });

        // Update new proof status based on success or error
        match proof_bytes_res {
            Ok(proof_bytes) => {
                info!("Completed proof {} in {} minutes", proof_request, minutes);
                state
                    .proof_store_client
                    .proof_status_update(
                        proof_request.request_id,
                        ExecutionStatus::Executed.into(),
                        Some(proof_bytes),
                    )
                    .await;
            }
            Err(e) => {
                let error_msg =
                    format!("Error proving {} at minute {}: {e}", proof_request, minutes);

                // If a contemplant errors while making a proof it should seppuku.
                // This message will force the program to exit gracefully.
                exit_sender
                    .send(error_msg)
                    .await
                    .context("Send exit error message to main thread")
                    .unwrap();

                state
                    .proof_store_client
                    .proof_status_update(
                        proof_request.request_id,
                        ExecutionStatus::Unexecutable.into(),
                        None,
                    )
                    .await;
            }
        };

        // send message to stop assessor
        if let Err(err) = assessor_shutdown_tx.send(true) {
            error!("Error sending shutdown signal to assessor: {err}");
        }
    });
}
