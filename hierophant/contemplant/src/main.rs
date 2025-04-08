mod config;
mod types;

use alloy_primitives::{B256, hex};

use crate::config::Config;
use crate::types::ProofStatus;
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use log::{error, info};
// use network_lib::{
//     AggProofRequest, AppError, GenericProofRequest, ProofStatus, ProofStore, ProofType,
//     SpanProofRequest, WorkerAggProofRequest, WorkerConfig, WorkerInfo, WorkerSpanProofRequest,
// };
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use sp1_sdk::{
    CudaProver, Prover, ProverClient, SP1_CIRCUIT_VERSION, SP1Proof, SP1ProofMode,
    SP1ProofWithPublicValues, SP1ProvingKey, SP1Stdin, SP1VerifyingKey,
    network::proto::network::{ExecutionStatus, FulfillmentStatus},
    utils,
};
use std::{collections::HashMap, env, fmt::Display, sync::Arc};
use tokio::{
    sync::RwLock,
    time::{Duration, sleep},
};
use tower_http::limit::RequestBodyLimitLayer;

const WORKER_REGISTER_ENDPOINT: &str = "worker";

const RANGE_ELF_EMBEDDED: &[u8] = include_bytes!("../../elf/range-elf-embedded");
const AGG_ELF: &[u8] = include_bytes!("../../elf/aggregation-elf");

#[derive(Clone)]
pub struct WorkerState {
    config: Config,
    cuda_prover: Arc<CudaProver>,
}

// impl Display for WorkerState {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.name)
//     }
// }

#[tokio::main]
async fn main() -> Result<()> {
    // Enable logging.
    unsafe {
        env::set_var("RUST_LOG", "info");
    }

    let config = tokio::fs::read_to_string("contemplant.toml")
        .await
        .context("read contemplant.toml file")?;

    let config: Config = toml::de::from_str(&config).context("parse config")?;

    // Set up the SP1 SDK logger.
    utils::setup_logger();

    let cuda_prover = Arc::new(ProverClient::builder().cuda().build());
    // TODO: might not need to initialize these here
    let (range_pk, range_vk) = cuda_prover.setup(RANGE_ELF_EMBEDDED);
    let (agg_pk, _agg_vk) = cuda_prover.setup(AGG_ELF);

    // let proof_store = Arc::new(RwLock::new(HashMap::new()));

    // Set the aggregation proof type based on environment variable. Default to groth16.
    // let agg_proof_mode = match env::var("AGG_PROOF_MODE") {
    //     Ok(proof_type) if proof_type.to_lowercase() == "plonk" => SP1ProofMode::Plonk,
    //     _ => SP1ProofMode::Groth16,
    // };

    // let worker_config = WorkerConfig {
    //     range_vk: Arc::new(range_vk),
    //     range_pk: Arc::new(range_pk),
    //     agg_pk: Arc::new(agg_pk),
    //     agg_proof_mode,
    //     proof_store,
    //     prover,
    // };

    let worker_state = WorkerState {
        cuda_prover,
        config: config.clone(),
    };

    let app = Router::new()
        .route("/request_proof", post(request_proof))
        .route("/status/:proof_id", get(get_proof_status))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(102400 * 1024 * 1024))
        .with_state(worker_state);

    let port = config.port.to_string();
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
        .await
        .unwrap();
    let local_addr = listener.local_addr().unwrap();

    // Send a "ready" notification to the coordinator
    tokio::spawn(async move {
        // Give this server a moment to fully initialize
        sleep(Duration::from_secs(1)).await;

        let client = Client::new();

        // Attempt to register with the coordinator
        match client
            .put(format!(
                "{}/{WORKER_REGISTER_ENDPOINT}",
                config.coordinator_address
            ))
            .json(&config)
            .send()
            .await
        {
            Ok(response) => {
                if response.status().is_success() {
                    info!(
                        "Successfully registered with coordinator at {}",
                        config.coordinator_address
                    );
                } else {
                    error!(
                        "Failed to register with coordinator {}: HTTP {}",
                        config.coordinator_address,
                        response.status()
                    );
                }
            }
            Err(err) => {
                error!("Failed to connect to coordinator: {}", err);
            }
        }
    });

    info!(
        "Worker server listening on {}",
        listener.local_addr().unwrap()
    );
    axum::serve(listener, app).await?;
    Ok(())
}

// TODO:
#[derive(Serialize, Deserialize)]
struct ProofRequest {}

async fn request_proof(
    State(state): State<WorkerState>,
    Json(payload): Json<ProofRequest>,
) -> axum::response::Result<StatusCode> {
    // TODO:
    Ok(StatusCode::OK)
}

async fn get_proof_status(
    State(state): State<WorkerState>,
    Path(proof_id): Path<String>,
) -> axum::response::Result<(StatusCode, Json<ProofStatus>)> {
    let proof_id_bytes = hex::decode(&proof_id)?;
    let proof_id = B256::from_slice(&proof_id_bytes);
    info!("Received proof status request: {:?}", proof_id);

    let proof_store = state.proof_store.read().await;

    let proof_status = match proof_store.get(&proof_id) {
        Some(status) => {
            info!("Proof status of {proof_id}: {}", status);
            status
        }
        None => {
            error!("Proof {} not found", proof_id);
            ProofStatus::unexecutable()
        }
    };

    Ok((StatusCode::OK, Json(status)))
}
// async fn request_span_proof(
//     State(state): State<WorkerConfig>,
//     Json(payload): Json<WorkerSpanProofRequest>,
// ) -> Result<StatusCode, AppError> {
//     info!(
//         "Received proof request for range {}, {} with id {}",
//         payload.start, payload.end, payload.proof_id
//     );
//
//     let initial_status = ProofStatus {
//         fulfillment_status: FulfillmentStatus::Assigned.into(),
//         execution_status: ExecutionStatus::Unexecuted.into(),
//         proof: Vec::new(),
//     };
//     state
//         .proof_store
//         .write()
//         .await
//         .insert(payload.proof_id, initial_status);
//
//     locally_prove(
//         state.clone(),
//         payload.mock_mode,
//         payload.proof_id,
//         payload.into(),
//     )
//     .await?;
//
//     Ok(StatusCode::OK)
// }

// async fn request_agg_proof(
//     State(state): State<WorkerConfig>,
//     Json(payload): Json<WorkerAggProofRequest>,
// ) -> Result<StatusCode, AppError> {
//     info!("Received agg proof request with id {:?}", payload.proof_id);
//
//     let initial_status = ProofStatus {
//         fulfillment_status: FulfillmentStatus::Assigned.into(),
//         execution_status: ExecutionStatus::Unexecuted.into(),
//         proof: Vec::new(),
//     };
//
//     state
//         .proof_store
//         .write()
//         .await
//         .insert(payload.proof_id, initial_status);
//
//     let (mock_mode, proof_id, generic_proof_request) = payload.split_to_generic();
//     locally_prove(state.clone(), mock_mode, proof_id, generic_proof_request).await?;
//
//     Ok(StatusCode::OK)
// }
//
// async fn get_proof_status(
//     State(state): State<WorkerConfig>,
//     Path(proof_id): Path<String>,
// ) -> Result<(StatusCode, Json<ProofStatus>), AppError> {
//     let proof_id_bytes = hex::decode(&proof_id)?;
//     let proof_id = B256::from_slice(&proof_id_bytes);
//     info!("Received proof status request: {:?}", proof_id);
//
//     // check if this is a proof we're generating locally.  Otherwise check the network for it
//     let proof_store = state.proof_store.read().await;
//     match proof_store.get(&proof_id) {
//         Some(status) => {
//             info!("Proof status of {proof_id}: {}", status);
//             Ok((
//                 StatusCode::OK,
//                 Json(ProofStatus {
//                     fulfillment_status: status.fulfillment_status,
//                     execution_status: status.execution_status,
//                     proof: status.proof.clone(),
//                 }),
//             ))
//         }
//         None => {
//             error!("Proof {} not found locally", proof_id);
//             Ok((
//                 StatusCode::OK,
//                 Json(ProofStatus {
//
//                     fulfillment_status: FulfillmentStatus::UnspecifiedFulfillmentStatus.into(),
//                     execution_status: ExecutionStatus::UnspecifiedExecutionStatus.into(),
//                     proof: vec![],
//                 }),
//             ))
//         }
//     }
// }
//
// // spawns a process that creates a proof locally
// // runs proof in a background thread.  Only needs to be async because of
// // proof_store.write().await
// async fn locally_prove(
//     state: WorkerConfig,
//     mock_mode: bool,
//     proof_id: B256,
//     proof_request: GenericProofRequest,
// ) -> Result<(), AppError> {
//     tokio::spawn(async move {
//         let start_time = tokio::time::Instant::now();
//
//         let (proof_type, proof_res) = match proof_request {
//             GenericProofRequest::Span(proof_request) => {
//                 match mock_mode {
//                     true => {
//                         info!("computing mock span proof with id {:?}", proof_id);
//                         let proof = mock_span_proof(proof_request, &state.range_pk).await;
//                         ("mock span", proof)
//                     }
//                     false => {
//                         info!("Generating stdin for span proof {}", proof_id);
//                         let sp1_stdin = match generate_span_stdin(&proof_request).await {
//                             Ok(stdin) => stdin,
//                             Err(e) => {
//                                 error!("Span proof setup error: {}", e);
//                                 // TODO: will this propagate the error?
//                                 return;
//                             }
//                         };
//                         info!("computing span proof {}", proof_id);
//                         // the cuda prover keeps state of the last `setup()` that was called on it.
//                         // You must call `setup()` then `prove` *each* time you intend to
//                         // prove a certain program
//                         let (proving_key, _) = state.prover.setup(RANGE_ELF);
//                         let proof = state
//                             .prover
//                             .prove(&proving_key, &sp1_stdin)
//                             .compressed()
//                             .run();
//                         ("span", proof)
//                     }
//                 }
//             }
//             GenericProofRequest::Agg(proof_request) => {
//                 match mock_mode {
//                     true => {
//                         info!("computing mock agg proof with id {:?}", proof_id);
//                         let proof = mock_agg_proof(
//                             proof_request,
//                             &state.range_vk,
//                             &state.agg_pk,
//                             state.agg_proof_mode,
//                         )
//                         .await;
//                         ("mock agg", proof)
//                     }
//                     false => {
//                         info!("Generating stdin for agg proof {}", proof_id);
//                         let sp1_stdin =
//                             match generate_agg_stdin(state.range_vk, proof_request).await {
//                                 Ok(stdin) => stdin,
//                                 Err(e) => {
//                                     error!("Agg proof setup error: {}", e);
//                                     // TODO: will this propagate the error?
//                                     return;
//                                 }
//                             };
//                         info!("computing agg proof {}", proof_id);
//                         // the cuda prover keeps state of the last `setup()` that was called on it.
//                         // You must call `setup()` then `prove` *each* time you intend to
//                         // prove a certain program
//                         let (proving_key, _) = state.prover.setup(AGG_ELF);
//                         let proof = state.prover.prove(&proving_key, &sp1_stdin).groth16().run();
//
//                         ("agg", proof)
//                     }
//                 }
//             }
//         };
//
//         let proof_status = match proof_res {
//             // proof is done, can return it
//             Ok(proof) => {
//                 match proof.proof {
//                     SP1Proof::Compressed(_) => {
//                         // If it's a compressed proof, we need to serialize the entire struct with bincode.
//                         // Note: We're re-serializing the entire struct with bincode here, but this is fine
//                         // because we're on localhost and the size of the struct is small.
//                         let proof_bytes = bincode::serialize(&proof).unwrap();
//                         ProofStatus {
//                             fulfillment_status: FulfillmentStatus::Fulfilled.into(),
//                             execution_status: ExecutionStatus::Executed.into(),
//                             proof: proof_bytes,
//                         }
//                     }
//                     SP1Proof::Groth16(_) => {
//                         // If it's a groth16 proof, we need to get the proof bytes that we put on-chain.
//                         let proof_bytes = proof.bytes();
//                         ProofStatus {
//                             fulfillment_status: FulfillmentStatus::Fulfilled.into(),
//                             execution_status: ExecutionStatus::Executed.into(),
//                             proof: proof_bytes,
//                         }
//                     }
//                     SP1Proof::Plonk(_) => {
//                         // If it's a plonk proof, we need to get the proof bytes that we put on-chain.
//                         let proof_bytes = proof.bytes();
//                         ProofStatus {
//                             fulfillment_status: FulfillmentStatus::Fulfilled.into(),
//                             execution_status: ExecutionStatus::Executed.into(),
//                             proof: proof_bytes,
//                         }
//                     }
//                     _ => {
//                         error!("unknown proof type: {proof:?}");
//                         ProofStatus {
//                             fulfillment_status: FulfillmentStatus::Unfulfillable.into(),
//                             execution_status: ExecutionStatus::Unexecutable.into(),
//                             proof: vec![],
//                         }
//                         // return Err(AppError(anyhow::anyhow!("unknown proof type: {proof:?}")));
//                     }
//                 }
//             }
//             Err(e) => {
//                 error!("error proving {e}");
//                 ProofStatus {
//                     fulfillment_status: FulfillmentStatus::Unfulfillable.into(),
//                     execution_status: ExecutionStatus::Unexecutable.into(),
//                     proof: vec![],
//                 }
//                 // return Err(AppError(anyhow::anyhow!("error proving {e}")));
//             }
//         };
//
//         info!("{proof_type} proof {proof_status}. id {proof_id}");
//         let minutes = start_time.elapsed().as_secs_f64() / 60.0;
//         info!("Time to complete {proof_type} proof: {} minutes", minutes);
//
//         // update proof store
//         state
//             .proof_store
//             .write()
//             .await
//             .insert(proof_id, proof_status);
//     });
//
//     Ok(())
// }
//
// async fn generate_agg_stdin(
//     range_vk: Arc<SP1VerifyingKey>,
//     payload: AggProofRequest,
// ) -> Result<SP1Stdin> {
//     let mut proofs_with_pv: Vec<SP1ProofWithPublicValues> = payload
//         .subproofs
//         .iter()
//         .map(|sp| bincode::deserialize(sp).unwrap())
//         .collect();
//
//     let boot_infos: Vec<BootInfoStruct> = proofs_with_pv
//         .iter_mut()
//         .map(|proof| proof.public_values.read())
//         .collect();
//
//     let proofs: Vec<SP1Proof> = proofs_with_pv
//         .iter_mut()
//         .map(|proof| proof.proof.clone())
//         .collect();
//
//     let l1_head_bytes = match payload.head.strip_prefix("0x") {
//         Some(hex_str) => match hex::decode(hex_str) {
//             Ok(bytes) => bytes,
//             Err(e) => {
//                 error!("Failed to decode L1 head hex string: {}", e);
//                 return Err(anyhow::anyhow!(
//                     "Failed to decode L1 head hex string: {}",
//                     e
//                 ));
//             }
//         },
//         None => {
//             error!("Invalid L1 head format: missing 0x prefix");
//             return Err(anyhow::anyhow!("Invalid L1 head format: missing 0x prefix"));
//         }
//     };
//
//     let l1_head: [u8; 32] = match l1_head_bytes.clone().try_into() {
//         Ok(array) => array,
//         Err(_) => {
//             error!(
//                 "Invalid L1 head length: expected 32 bytes, got {}",
//                 l1_head_bytes.len()
//             );
//             return Err(anyhow::anyhow!(
//                 "Invalid L1 head length: expected 32 bytes, got {}",
//                 l1_head_bytes.len()
//             ));
//         }
//     };
//
//     let fetcher = match OPSuccinctDataFetcher::new_with_rollup_config(RunContext::Docker).await {
//         Ok(f) => f,
//         Err(e) => {
//             error!("Failed to create fetcher: {}", e);
//             return Err(anyhow::anyhow!("Failed to create fetcher: {}", e));
//         }
//     };
//
//     let headers = match fetcher
//         .get_header_preimages(&boot_infos, l1_head.into())
//         .await
//     {
//         Ok(h) => h,
//         Err(e) => {
//             error!("Failed to get header preimages: {}", e);
//             return Err(anyhow::anyhow!("Failed to get header preimages: {}", e));
//         }
//     };
//
//     let sp1_stdin =
//         match get_agg_proof_stdin(proofs, boot_infos, headers, &range_vk, l1_head.into()) {
//             Ok(s) => s,
//             Err(e) => {
//                 error!("Failed to get agg proof stdin: {}", e);
//                 return Err(anyhow::anyhow!("Failed to get agg proof stdin: {}", e));
//             }
//         };
//
//     Ok(sp1_stdin)
// }
//
// async fn generate_span_stdin(payload: &SpanProofRequest) -> Result<SP1Stdin> {
//     let fetcher = match OPSuccinctDataFetcher::new_with_rollup_config(RunContext::Docker).await {
//         Ok(f) => f,
//         Err(e) => {
//             error!("Failed to create data fetcher: {}", e);
//             return Err(e);
//         }
//     };
//
//     let host_args = match fetcher
//         .get_host_args(
//             payload.start,
//             payload.end,
//             None,
//             ProgramType::Multi,
//             CacheMode::DeleteCache,
//         )
//         .await
//     {
//         Ok(cli) => cli,
//         Err(e) => {
//             error!("Failed to get host CLI args: {}", e);
//             return Err(anyhow::anyhow!("Failed to get host CLI args: {}", e));
//         }
//     };
//
//     let mem_kv_store = start_server_and_native_client(host_args).await?;
//
//     let sp1_stdin = match get_proof_stdin(mem_kv_store) {
//         Ok(stdin) => stdin,
//         Err(e) => {
//             error!("Failed to get proof stdin: {}", e);
//             return Err(anyhow::anyhow!("Failed to get proof stdin: {}", e));
//         }
//     };
//
//     Ok(sp1_stdin)
// }
//
// async fn mock_agg_proof(
//     payload: AggProofRequest,
//     range_vk: &SP1VerifyingKey,
//     agg_pk: &SP1ProvingKey,
//     agg_proof_mode: SP1ProofMode,
// ) -> Result<SP1ProofWithPublicValues> {
//     let mut proofs_with_pv: Vec<SP1ProofWithPublicValues> = payload
//         .subproofs
//         .iter()
//         .map(|sp| bincode::deserialize(sp).unwrap())
//         .collect();
//
//     let boot_infos: Vec<BootInfoStruct> = proofs_with_pv
//         .iter_mut()
//         .map(|proof| proof.public_values.read())
//         .collect();
//
//     let proofs: Vec<SP1Proof> = proofs_with_pv
//         .iter_mut()
//         .map(|proof| proof.proof.clone())
//         .collect();
//
//     let l1_head_bytes = match hex::decode(
//         payload
//             .head
//             .strip_prefix("0x")
//             .expect("Invalid L1 head, no 0x prefix."),
//     ) {
//         Ok(bytes) => bytes,
//         Err(e) => {
//             error!("Failed to decode L1 head: {}", e);
//             return Err(anyhow::anyhow!("Failed to decode L1 head: {}", e));
//         }
//     };
//     let l1_head: [u8; 32] = l1_head_bytes.try_into().unwrap();
//
//     let fetcher = match OPSuccinctDataFetcher::new_with_rollup_config(RunContext::Docker).await {
//         Ok(f) => f,
//         Err(e) => {
//             error!("Failed to create data fetcher: {}", e);
//             return Err(e);
//         }
//     };
//     let headers = match fetcher
//         .get_header_preimages(&boot_infos, l1_head.into())
//         .await
//     {
//         Ok(h) => h,
//         Err(e) => {
//             error!("Failed to get header preimages: {}", e);
//             return Err(e);
//         }
//     };
//
//     let stdin = match get_agg_proof_stdin(proofs, boot_infos, headers, range_vk, l1_head.into()) {
//         Ok(s) => s,
//         Err(e) => {
//             error!("Failed to get aggregation proof stdin: {}", e);
//             return Err(e);
//         }
//     };
//
//     // Note(ratan): In a future version of the server which only supports mock proofs, Arc<MockProver> should be used to reduce memory usage.
//     let prover = ProverClient::builder().mock().build();
//     let proof = match prover
//         .prove(agg_pk, &stdin)
//         .mode(agg_proof_mode)
//         .deferred_proof_verification(false)
//         .run()
//     {
//         Ok(p) => p,
//         Err(e) => {
//             error!("Failed to generate proof: {}", e);
//             return Err(e);
//         }
//     };
//
//     Ok(proof)
// }
//
// async fn mock_span_proof(
//     payload: SpanProofRequest,
//     range_pk: &SP1ProvingKey,
// ) -> Result<SP1ProofWithPublicValues> {
//     let fetcher = match OPSuccinctDataFetcher::new_with_rollup_config(RunContext::Docker).await {
//         Ok(f) => f,
//         Err(e) => {
//             error!("Failed to create data fetcher: {}", e);
//             return Err(e);
//         }
//     };
//
//     let host_args = match fetcher
//         .get_host_args(
//             payload.start,
//             payload.end,
//             None,
//             ProgramType::Multi,
//             CacheMode::DeleteCache,
//         )
//         .await
//     {
//         Ok(cli) => cli,
//         Err(e) => {
//             error!("Failed to get host CLI args: {}", e);
//             return Err(e);
//         }
//     };
//
//     let oracle = start_server_and_native_client(host_args.clone()).await?;
//
//     let sp1_stdin = match get_proof_stdin(oracle) {
//         Ok(stdin) => stdin,
//         Err(e) => {
//             error!("Failed to get proof stdin: {}", e);
//             return Err(e);
//         }
//     };
//
//     // Note(ratan): In a future version of the server which only supports mock proofs, Arc<MockProver> should be used to reduce memory usage.
//     let prover = ProverClient::builder().mock().build();
//     let (pv, _) = prover.execute(RANGE_ELF, &sp1_stdin).run().unwrap();
//
//     let proof = SP1ProofWithPublicValues::create_mock_proof(
//         range_pk,
//         pv.clone(),
//         SP1ProofMode::Compressed,
//         SP1_CIRCUIT_VERSION,
//     );
//
//     Ok(proof)
// }
