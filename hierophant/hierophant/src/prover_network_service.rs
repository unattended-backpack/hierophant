use crate::hierophant_state::{HierophantState, ProofRequestData, VkHash};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use crate::proof_router::worker_state::WorkerState;
use alloy_primitives::{Address, B256};
use log::{error, info};
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
        info!("Requested program with vk_hash: 0x{vk_hash_hex}",);

        // get program
        let maybe_program = self.state.program_store.lock().await.get(&vk_hash).cloned();

        match maybe_program {
            Some(_) => {
                info!("Program with vk_hash 0x{vk_hash_hex} found");
            }
            None => {
                info!("Program with vk_hash 0x{vk_hash_hex} not found");
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

        println!("Responding with nonce: {}", response.nonce);

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
        println!("CreateProgram request details:");
        println!("  Nonce: {}", body.nonce);
        println!("  VK Hash: 0x{}", vk_hash_hex);
        println!("  VK size: {} bytes", body.vk.len());
        println!("  Program URI: {}", body.program_uri);

        // TODO: should owner be the requesting client or the Heirophant's pub key?
        let owner = self.state.config.pub_key;
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // TODO: newtype ProofName that is just a prefix of "proof:<vk_hash>" for easier logging.
        // include nice methods for going to/from uuid
        // Can also extend this for all artifact types
        let name = None;

        let program = Program {
            vk_hash: vk_hash.clone().into(),
            vk: body.vk,
            program_uri: body.program_uri,
            owner: (*owner).to_vec(),
            created_at,
            name,
        };
        info!("created program with vk_hash 0x{vk_hash_hex}");

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
        println!("Signature: 0x{}", hex::encode(&req.signature));

        // Create the response
        let response = CreateProgramResponse {
            tx_hash,
            body: Some(CreateProgramResponseBody {}),
        };

        println!(
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
        let vk_hash: VkHash = body.vk_hash.into();
        let vk_hash_hex = vk_hash.to_hex_string();
        println!("RequestProof request details:");
        println!("  Nonce: {}", body.nonce);
        println!("  VK Hash: 0x{}", vk_hash_hex);
        println!("  Version: {}", body.version);
        println!("  Mode: {}", body.mode);
        println!("  Strategy: {}", body.strategy);
        println!("  Stdin URI: {}", body.stdin_uri);
        println!("  Deadline: {}", body.deadline);
        println!("  Cycle Limit: {}", body.cycle_limit);
        println!("  Gas Limit: {}", body.gas_limit);

        // Log the signature
        println!("Signature: 0x{}", hex::encode(&req.signature));

        // Generate a mock request ID (this would typically be a unique identifier for the proof request) TODO: is the proof itself unique on vk_hash?? (we're currently assuming it is in the proof_cache) an in heirophant_state
        let mut request_id = vec![0u8; 32];
        let uuid = Uuid::new_v4();
        let uuid_bytes = uuid.as_bytes();
        request_id[0..16].copy_from_slice(uuid_bytes);

        // route the proof!!
        self.state.proof_router.route_proof();

        // Generate a mock transaction hash
        let mut tx_hash = vec![0u8; 32]; // 32-byte transaction hash
        for (i, &byte) in req.signature.iter().take(32).enumerate() {
            tx_hash[i] = byte;
        }

        // Store proof request data for status checks
        let deadline = body.deadline;

        // TODO: why?
        // Store in our request database
        let proof_request_data = ProofRequestData {
            tx_hash: tx_hash.clone(),
            deadline,
            fulfillment_status: FulfillmentStatus::Pending,
            execution_status: ExecutionStatus::Unexecuted,
            proof_uri: None,
            fulfill_tx_hash: None,
            public_values_hash: None,
        };

        self.state
            .proof_requests
            .lock()
            .await
            .insert(vk_hash, proof_request_data);

        // Create the response
        let response = RequestProofResponse {
            tx_hash,
            body: Some(RequestProofResponseBody { request_id }),
        };

        println!("Responding with:");
        println!("  tx_hash: 0x{}", hex::encode(&response.tx_hash));
        println!(
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

        // Log request ID
        println!(
            "Requested status for request_id: 0x{}",
            hex::encode(&req.request_id)
        );

        self.state.proof_router.get_proof_status(&req.request_id)
    }
}
