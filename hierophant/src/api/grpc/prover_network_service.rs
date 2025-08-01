use crate::artifact_store::ArtifactUri;
use crate::hierophant_state::{HierophantState, VkHash};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, FulfillmentStatus,
    GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use crate::proof::ProofStatus;
use alloy_primitives::{Address, B256};
use axum::body::Bytes;
use log::{debug, error, info, warn};
use sp1_sdk::network::proto::network::ExecutionStatus;
use sp1_sdk::network::proto::{artifact::ArtifactType, network::ProofMode};
use sp1_sdk::proof::ProofFromNetwork;
use sp1_sdk::{Prover, SP1ProofWithPublicValues};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{str::FromStr, sync::Arc};
use tokio::time::Instant;
use tonic::{Request, Response, Status};

// Our ProverNetwork service implementation
pub struct ProverNetworkService {
    state: Arc<HierophantState>,
}

impl ProverNetworkService {
    pub fn new(state: Arc<HierophantState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ProverNetwork for ProverNetworkService {
    async fn get_program(
        &self,
        request: Request<GetProgramRequest>,
    ) -> Result<Response<GetProgramResponse>, Status> {
        let req = request.into_inner();

        let vk_hash: VkHash = req.vk_hash.into();
        let vk_hash_hex = vk_hash.to_hex_string();

        info!("Requested program with vk_hash: {vk_hash_hex}",);

        // get program
        let maybe_program = self.state.program_store.lock().await.get(&vk_hash).cloned();

        match maybe_program {
            Some(program) => {
                debug!("Program with vk_hash {vk_hash_hex} found");
                // Create the response with the program
                let response = GetProgramResponse {
                    program: Some(program),
                };
                Ok(Response::new(response))
            }
            None => {
                info!("Program with vk_hash {vk_hash_hex} not found");
                Err(Status::not_found("program not found"))
            }
        }
    }

    async fn get_nonce(
        &self,
        request: Request<GetNonceRequest>,
    ) -> Result<Response<GetNonceResponse>, Status> {
        let req = request.into_inner();

        let address: Address = match req.address.as_slice().try_into() {
            Ok(address) => address,
            Err(e) => {
                let error_msg =
                    format!("Can't parse {} as Address: {e}", hex::encode(&req.address));
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        // Nonces not yet implemented
        let nonce = 0;

        debug!("Nonce of address {address} is {nonce}");

        // Create the response
        let response = GetNonceResponse { nonce };

        Ok(Response::new(response))
    }

    async fn create_program(
        &self,
        request: Request<CreateProgramRequest>,
    ) -> Result<Response<CreateProgramResponse>, Status> {
        let req = request.into_inner();

        let body = match req.body {
            Some(body) => body,
            None => {
                // The client only ever supplies Some(program) [as of sp1-sdk v4.1.3].
                // It's not clear why it would ever be None.
                let error_msg = format!(
                    "No program supplied in body of CreateProgramRequest with signature {}",
                    hex::encode(&req.signature)
                );
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        let vk_hash: VkHash = body.vk_hash.into();
        let vk_hash_hex = vk_hash.to_hex_string();

        // Extract and log the body contents if present
        info!("CreateProgram request details:");
        info!("  Nonce: {}", body.nonce);
        info!("  VK Hash: 0x{}", vk_hash_hex);
        info!("  VK size: {} bytes", body.vk.len());
        info!("  Program URI: {}", body.program_uri);

        // TODO: signing impl
        let owner = self.state.config.pub_key;
        let created_at = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(x) => x.as_secs(),
            Err(e) => {
                let err = format!("Error constructing created_at time: {e}");
                return Err(Status::internal(err));
            }
        };

        let name = None;

        let program = Program {
            vk_hash: vk_hash.clone().into(),
            vk: body.vk,
            program_uri: body.program_uri,
            owner: (*owner).to_vec(),
            created_at,
            name,
        };
        debug!("created program with vk_hash {vk_hash_hex}");

        self.state
            .program_store
            .lock()
            .await
            .insert(vk_hash, program);

        // Generate a mock transaction hash (in a real implementation, this would be from the blockchain)
        let tx_hash = (*B256::random()).to_vec();

        // Log the signature
        debug!("Signature: 0x{}", hex::encode(&req.signature));

        // Create the response
        let response = CreateProgramResponse {
            tx_hash,
            body: Some(CreateProgramResponseBody {}),
        };

        debug!(
            "Responding with tx_hash: 0x{}",
            hex::encode(&response.tx_hash)
        );

        Ok(Response::new(response))
    }

    async fn request_proof(
        &self,
        request: Request<RequestProofRequest>,
    ) -> Result<Response<RequestProofResponse>, Status> {
        let req = request.into_inner();

        let body = match req.body {
            Some(body) => body,
            None => {
                // The client only ever supplies Some(program) [as of sp1-sdk v4.1.3].
                // It's not clear why it would ever be None.
                let error_msg = format!(
                    "No program supplied in body of RequestProof with signature {}",
                    hex::encode(&req.signature)
                );
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        let request_id = B256::random();
        // Extract and log the body
        let vk_hash: VkHash = body.vk_hash.clone().into();
        let vk_hash_hex = vk_hash.to_hex_string();
        info!("RequestProof details:");
        info!("  Nonce: {}", body.nonce);
        info!("  VK Hash: {}", vk_hash_hex);
        info!("  Version: {}", body.version);
        info!(
            "  Mode: {}",
            ProofMode::try_from(body.mode)
                .unwrap_or(ProofMode::UnspecifiedProofMode)
                .as_str_name()
        );
        info!("  Strategy: {}", body.strategy);
        info!("  Stdin URI: {}", body.stdin_uri);
        info!("  Deadline: {}", body.deadline);
        info!("  Cycle Limit: {}", body.cycle_limit);
        info!("  Gas Limit: {}", body.gas_limit);
        info!("Assigned proof request id {request_id}");

        // Log the signature
        debug!("Signature: 0x{}", hex::encode(&req.signature));

        let stdin_uri = match ArtifactUri::from_str(&body.stdin_uri) {
            Ok(uri) => uri,
            Err(e) => {
                let error_msg =
                    format!("Error parsing stdin_uri from proof request {request_id}: {e}");
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        // get program with this vk_hash
        let program_uri = match self.state.program_store.lock().await.get(&vk_hash) {
            Some(prog) => prog.program_uri.clone(),
            None => {
                let error_msg = format!("Program with vk_hash {vk_hash_hex} not found in db");
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        // parse program_uri as ArtifactUri
        let program_uri = match ArtifactUri::from_str(&program_uri) {
            Ok(uri) => uri,
            Err(e) => {
                let error_msg =
                    format!("Error parsing program_uri {program_uri} as ArtifactUri {e}");
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        let mode = match ProofMode::try_from(body.mode) {
            Ok(m) => m,
            Err(e) => {
                let error_msg = format!("Error parsing {} as ProofMode: {e}", body.mode);
                error!("{error_msg}");
                return Err(Status::invalid_argument(error_msg));
            }
        };

        let start = Instant::now();
        // route the proof to a worker to be completed
        if let Err(e) = self
            .state
            .proof_router
            .route_proof(
                request_id,
                program_uri,
                stdin_uri,
                mode,
                self.state.artifact_store_client.clone(),
            )
            .await
        {
            let error_msg = format!("Internal error routing proof request {request_id}: {e}");
            error!("{error_msg}");
            return Err(Status::internal(error_msg));
        }
        info!(
            "Took {} seconds to route proof",
            start.elapsed().as_secs_f32()
        );

        // Generate a mock transaction hash
        let mut tx_hash = vec![0u8; 32]; // 32-byte transaction hash
        for (i, &byte) in req.signature.iter().take(32).enumerate() {
            tx_hash[i] = byte;
        }

        // generate artifact uri for the proof for later artifact saving (in
        // get_proof_request_status)
        let artifact_uri = match self
            .state
            .artifact_store_client
            .create_artifact(ArtifactType::Proof)
            .await
        {
            Ok(uri) => uri,
            Err(e) => {
                error!("{e}");
                return Err(Status::internal(e.to_string()));
            }
        };

        self.state
            .proof_requests
            .lock()
            .await
            .insert(request_id, (artifact_uri.clone(), body));
        debug!(
            "Assigned proof to be generated from request {request_id} to artifact_uri {artifact_uri}"
        );

        // Create the response
        let response = RequestProofResponse {
            tx_hash,
            body: Some(RequestProofResponseBody {
                request_id: request_id.to_vec(),
            }),
        };

        debug!("Responding with:");
        debug!("  tx_hash: 0x{}", hex::encode(&response.tx_hash));
        // debug!(
        //     "  request_id: 0x{}",
        //     hex::encode(&response.body.as_ref().unwrap().request_id)
        // );

        Ok(Response::new(response))
    }

    // TODO: refactor this function into logical chunks
    async fn get_proof_request_status(
        &self,
        request: Request<GetProofRequestStatusRequest>,
    ) -> Result<Response<GetProofRequestStatusResponse>, Status> {
        let req = request.into_inner();
        let request_id = match req.request_id.clone().try_into() {
            Ok(id) => B256::new(id),
            Err(_) => {
                let error_msg = format!(
                    "Error parsing request_id 0x{} as B256.",
                    hex::encode(&req.request_id)
                );
                error!("{error_msg}");
                let response = lost_proof_response();
                return Ok(Response::new(response));
            }
        };

        // look up previously generated artifact_uri (proof_uri is made in request_proof)
        let (proof_uri, request_proof_request_body) = match self
            .state
            .proof_requests
            .lock()
            .await
            .get(&request_id)
        {
            Some(b) => b.clone(),
            None => {
                let error_msg = format!(
                    "Proof request {request_id} not found in proof_requests mapping.  This proof might not have been requested yet."
                );
                warn!("{error_msg}");

                let response = lost_proof_response();
                return Ok(Response::new(response));
            }
        };

        // If proof is already in artifact store, no need to request prover network.  Can return
        // early
        if let Ok(Some(_)) = self
            .state
            .artifact_store_client
            .get_artifact_bytes(proof_uri.clone())
            .await
        {
            info!(
                "Found proof with request id {request_id} in artifact store with uri {proof_uri}"
            );

            let request_tx_hash = vec![];
            let fulfill_tx_hash = None;
            let public_values_hash = None;

            let proof_download_address = format!(
                "http://{}:{}/{}",
                self.state.config.this_hierophant_ip, self.state.config.http_port, proof_uri
            );

            debug!("Responding with proof download address {proof_download_address}");

            let response = GetProofRequestStatusResponse {
                fulfillment_status: FulfillmentStatus::Fulfilled.into(),
                execution_status: ExecutionStatus::Executed.into(),
                request_tx_hash,
                deadline: request_proof_request_body.deadline,
                fulfill_tx_hash,
                // It's called proof_uri but the client is actually expecting the endpoint to hit
                // to download this proof
                proof_uri: Some(proof_download_address),
                public_values_hash,
            };

            return Ok(Response::new(response));
        };

        // Check the workers for the proof
        let proof_status = match self.state.proof_router.get_proof_status(request_id).await {
            Ok(status) => status,
            Err(e) => {
                let error_msg = format!("Error getting proof status {e}");
                error!("{error_msg}");

                let response = lost_proof_response();
                return Ok(Response::new(response));
            }
        };

        let assigned_status: i32 = FulfillmentStatus::Assigned.into();
        if proof_status.fulfillment_status != assigned_status {
            info!(
                "Proof request {request_id} with uri {proof_uri} not yet assigned: {proof_status}"
            );
        }

        // Succinct's prover network is integrated on-chain but ours isn't
        let request_tx_hash = vec![];
        let fulfill_tx_hash = None;
        let public_values_hash = None;

        let proof_download_address = format!(
            "http://{}:{}/{}",
            self.state.config.this_hierophant_ip, self.state.config.http_port, proof_uri
        );

        // if proof is complete, save it to disk as an artifact and mark the worker
        // as idle
        if !proof_status.proof.is_empty() {
            let proof: SP1ProofWithPublicValues =
                match bincode::deserialize::<ProofFromNetwork>(&proof_status.proof) {
                    Ok(p) => p.into(),
                    Err(e) => {
                        error!("{e}");
                        let response = lost_proof_response();
                        return Ok(Response::new(response));
                    }
                };

            let vkey = match self.state.get_vk(&request_id).await {
                Ok(vkey) => vkey,
                Err(e) => {
                    error!("{e}");
                    let response = lost_proof_response();
                    return Ok(Response::new(response));
                }
            };

            let proof_mode = match ProofMode::try_from(request_proof_request_body.mode) {
                Ok(mode) => mode,
                Err(e) => {
                    warn!(
                        "Proof request {request_id} with uri {proof_uri} has unknown ProofMode: {}.  Error: {e}",
                        request_proof_request_body.mode
                    );
                    let response = lost_proof_response();
                    return Ok(Response::new(response));
                }
            };

            // if we don't yet have the circuits for verifying groth16 proofs we'll have to make a
            // network call to download them, which takes awhile.  So for now just return "still
            // proving" to the caller
            let need_to_download_circuit = match proof_mode {
                ProofMode::Groth16 => {
                    let circuit_dir_exists =
                        sp1_sdk::install::groth16_circuit_artifacts_dir().exists();

                    if !circuit_dir_exists {
                        // start downloading verifying artifacts in a separate tread
                        tokio::spawn(async move {
                            info!("Starting to download groth16 circuit artifacts for verifying");
                            let path = sp1_sdk::install::try_install_circuit_artifacts("groth16");
                            info!(
                                "Downloaded groth16 circuit artifacts for verifying to {}",
                                path.to_str().unwrap_or("unknown")
                            );
                        });
                    }

                    !circuit_dir_exists
                }
                ProofMode::Plonk => {
                    let circuit_dir_exists =
                        sp1_sdk::install::plonk_circuit_artifacts_dir().exists();

                    if !circuit_dir_exists {
                        // start downloading verifying artifacts in a separate tread
                        tokio::spawn(async move {
                            info!("Starting to download plonk circuit artifacts for verifying");
                            let path = sp1_sdk::install::try_install_circuit_artifacts("plonk");
                            info!(
                                "Downloaded plonk circuit artifacts for verifying to {}",
                                path.to_str().unwrap_or("unknown")
                            );
                        });
                    }

                    !circuit_dir_exists
                }
                // for other proof types we don't need to download a circuit
                _ => false,
            };

            if need_to_download_circuit {
                // pretend like we're still waiting on a proof
                let response = pretend_still_waiting_proof_response(
                    request_proof_request_body.deadline,
                    &proof_download_address,
                );
                return Ok(Response::new(response));
            }

            // make sure the proof verifies
            if let Err(e) = self.state.cpu_prover.verify(&proof, &vkey) {
                warn!(
                    "Error verifying proof {request_id}: {e}.  Dropping worker and returning lost proof status"
                );

                // Drop worker who was assigned to this proof
                self.state
                    .proof_router
                    .worker_registry_client
                    .drop_worker_of_request(request_id)
                    .await;

                let response = lost_proof_response();
                return Ok(Response::new(response));
            }

            info!("Verified proof {request_id}!!");

            // mark that worker who returned a completed proof as idle
            if let Err(e) = self
                .state
                .proof_router
                .worker_registry_client
                .proof_complete(request_id)
                .await
            {
                error!("{e}");
                let response = lost_proof_response();
                return Ok(Response::new(response));
            }

            // save artifact to disk
            if let Err(e) = self
                .state
                .artifact_store_client
                .save_artifact(proof_uri.clone(), Bytes::from_owner(proof_status.proof))
                .await
            {
                error!("{e}");
                let response = lost_proof_response();
                return Ok(Response::new(response));
            }

            info!("Saved proof from request_id {request_id} to disk with uri {proof_uri}");
        }

        debug!("Responding with proof download address {proof_download_address}");

        let response = GetProofRequestStatusResponse {
            fulfillment_status: proof_status.fulfillment_status,
            execution_status: proof_status.execution_status,
            request_tx_hash,
            deadline: request_proof_request_body.deadline,
            fulfill_tx_hash,
            proof_uri: Some(proof_download_address),
            public_values_hash,
        };

        Ok(Response::new(response))
    }
}

fn lost_proof_response() -> GetProofRequestStatusResponse {
    let proof_status = ProofStatus::lost();
    GetProofRequestStatusResponse {
        fulfillment_status: proof_status.fulfillment_status,
        execution_status: proof_status.execution_status,
        request_tx_hash: vec![],
        deadline: 0,
        fulfill_tx_hash: None,
        proof_uri: None,
        public_values_hash: None,
    }
}

fn pretend_still_waiting_proof_response(
    deadline: u64,
    proof_download_address: &str,
) -> GetProofRequestStatusResponse {
    GetProofRequestStatusResponse {
        fulfillment_status: FulfillmentStatus::Assigned.into(),
        execution_status: ExecutionStatus::Unexecuted.into(),
        request_tx_hash: vec![],
        deadline,
        fulfill_tx_hash: None,
        proof_uri: Some(proof_download_address.into()),
        public_values_hash: None,
    }
}
