use crate::hierophant_state::{HierophantState, VkHash};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use crate::proof_router::worker_state::WorkerState;
use alloy_primitives::{Address, B256};
use axum::body::Bytes;
use log::{error, info};
use network_lib::ProofRequestId;
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::{Request, Response, Status};
use uuid::Uuid;

// Our ProverNetwork service implementation
#[derive(Debug)]
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
        info!("get_program called");
        let req = request.into_inner();

        let vk_hash: VkHash = req.vk_hash.into();
        let vk_hash_hex = vk_hash.to_hex_string();

        // Log vk_hash
        info!("Requested program with vk_hash: {vk_hash_hex}",);

        // get program
        let maybe_program = self.state.program_store.lock().await.get(&vk_hash).cloned();

        match maybe_program {
            Some(_) => {
                info!("Program with vk_hash {vk_hash_hex} found");
            }
            None => {
                info!("Program with vk_hash {vk_hash_hex} not found");
            }
        }

        // Create the response with the program
        let response = GetProgramResponse {
            program: maybe_program,
        };

        Ok(Response::new(response))
    }

    async fn get_nonce(
        &self,
        request: Request<GetNonceRequest>,
    ) -> Result<Response<GetNonceResponse>, Status> {
        info!("get_nonce called");
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

        let nonce = match self.state.nonces.lock().await.get(&address) {
            Some(nonce) => *nonce,
            None => 0,
        };

        info!("Nonce of address {address} is {nonce}");

        // Create the response
        let response = GetNonceResponse { nonce };

        info!("Responding with nonce: {}", response.nonce);

        Ok(Response::new(response))
    }

    async fn create_program(
        &self,
        request: Request<CreateProgramRequest>,
    ) -> Result<Response<CreateProgramResponse>, Status> {
        info!("create_program called");
        // TODO: might have to handle differently based on request.format
        // or does tonic handle this for us?
        /*
        pub enum MessageFormat {
            UnspecifiedFormat = 0,
            Json = 1,
            Binary = 2,
        }
        */

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

        // TODO: should owner be the requesting client or the Heirophant's pub key?
        let owner = self.state.config.pub_key;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let name = None;

        let program = Program {
            vk_hash: vk_hash.clone().into(),
            vk: body.vk,
            program_uri: body.program_uri,
            owner: (*owner).to_vec(),
            created_at,
            name,
        };
        info!("created program with vk_hash {vk_hash_hex}");

        self.state
            .program_store
            .lock()
            .await
            .insert(vk_hash, program);

        // Generate a mock transaction hash (in a real implementation, this would be from the blockchain)
        let tx_hash = (*B256::random()).to_vec();

        // TODO: verify VK?

        // TODO: who is signing this? Verify signature

        // TODO: increment nonce?

        // Log the signature
        info!("Signature: 0x{}", hex::encode(&req.signature));

        // Create the response
        let response = CreateProgramResponse {
            tx_hash,
            body: Some(CreateProgramResponseBody {}),
        };

        info!(
            "Responding with tx_hash: 0x{}",
            hex::encode(&response.tx_hash)
        );

        Ok(Response::new(response))
    }

    async fn request_proof(
        &self,
        request: Request<RequestProofRequest>,
    ) -> Result<Response<RequestProofResponse>, Status> {
        info!("request_proof called");
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

        // Extract and log the body
        let vk_hash: VkHash = body.vk_hash.clone().into();
        let vk_hash_hex = vk_hash.to_hex_string();
        info!("RequestProof request details:");
        info!("  Nonce: {}", body.nonce);
        info!("  VK Hash: {}", vk_hash_hex);
        info!("  Version: {}", body.version);
        info!("  Mode: {}", body.mode);
        info!("  Strategy: {}", body.strategy);
        info!("  Stdin URI: {}", body.stdin_uri);
        info!("  Deadline: {}", body.deadline);
        info!("  Cycle Limit: {}", body.cycle_limit);
        info!("  Gas Limit: {}", body.gas_limit);

        // Log the signature
        info!("Signature: 0x{}", hex::encode(&req.signature));

        let request_id = ProofRequestId::new(
            body.vk_hash.clone(),
            body.version.clone(),
            body.stdin_uri.clone(),
        );

        info!("Assigned proof request id {request_id}");

        // route the proof to a worker to be completed
        self.state.proof_router.route_proof(request_id);

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
        info!(
            "Assigned proof to be generated from request {request_id} to artifact_uri {artifact_uri}"
        );

        // Create the response
        let response = RequestProofResponse {
            tx_hash,
            body: Some(RequestProofResponseBody {
                request_id: request_id.into(),
            }),
        };

        info!("Responding with:");
        info!("  tx_hash: 0x{}", hex::encode(&response.tx_hash));
        info!(
            "  request_id: 0x{}",
            hex::encode(&response.body.as_ref().unwrap().request_id)
        );

        Ok(Response::new(response))
    }

    async fn get_proof_request_status(
        &self,
        request: Request<GetProofRequestStatusRequest>,
    ) -> Result<Response<GetProofRequestStatusResponse>, Status> {
        info!("get_proof_request_status called");
        let req = request.into_inner();
        let request_id = match req.request_id.clone().try_into() {
            Ok(id) => id,
            Err(e) => {
                let error_msg = format!(
                    "Error parsing request_id 0x{} as ProofRequestId (u64): {e}",
                    hex::encode(&req.request_id)
                );
                error!("{error_msg}");
                return Err(Status::not_found(error_msg));
            }
        };

        // Log request ID
        info!("Requested status for request_id: {request_id}");

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
                error!("{error_msg}");
                return Err(Status::not_found(error_msg));
            }
        };

        // TODO: try to get the proof from artifact_store first

        // Have the proof router find the proof
        let proof_status = match self.state.proof_router.get_proof_status(request_id).await {
            Ok(status) => status,
            Err(e) => {
                let error_msg = format!("Error getting proof status {e}");
                error!("{error_msg}");
                return Err(Status::not_found(error_msg));
            }
        };

        // if proof is complete, save it to disk as an artifact
        if !proof_status.proof.is_empty() {
            // save artifact to disk
            if let Err(e) = self
                .state
                .artifact_store_client
                .save_artifact(proof_uri.clone(), Bytes::from_owner(proof_status.proof))
                .await
            {
                error!("{e}");
                return Err(Status::internal(e.to_string()));
            }

            info!("Saved proof from request_id {request_id} to disk with uri {proof_uri}");
        }

        let response = GetProofRequestStatusResponse {
            fulfillment_status: proof_status.fulfillment_status,
            execution_status: proof_status.execution_status,
            request_tx_hash: todo!(),
            deadline: request_proof_request_body.deadline,
            fulfill_tx_hash: todo!(),
            proof_uri: Some(proof_uri.to_string()),
            // TODO: is this a hash of stdin?  Does that mean I have to load stdin_uri?
            public_values_hash: todo!(),
        };

        Ok(Response::new(response))
    }
}
