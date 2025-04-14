use crate::hierophant_state::{HierophantState, ProofRequestData, WorkerState, WorkerStatus};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
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

        // Log vk_hash
        println!("Requested vk_hash: 0x{}", hex::encode(&req.vk_hash));

        // Create a Program object that matches the expected structure
        let program = Program {
            // The vk_hash is what they're looking up by
            vk_hash: req.vk_hash.clone(),

            // Create a mock verification key (typically would be much larger)
            vk: vec![0xAA; 256], // 256 bytes of dummy VK data

            // A mock program URI
            program_uri: "hierophant://mock/program/1".to_string(),

            // Optional name
            name: Some("Mock Program".to_string()),

            // Mock owner (20 bytes, like an Ethereum address)
            owner: vec![0xBB; 20],

            // Current timestamp
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };

        // Create the response with the program
        let response = GetProgramResponse {
            program: Some(program),
        };

        println!("Responding with structured program data");

        // TODO: check if the program is here before replying with it.
        Err(Status::not_found("no program"))
        // Ok(Response::new(response))
    }

    async fn get_nonce(
        &self,
        request: Request<GetNonceRequest>,
    ) -> Result<Response<GetNonceResponse>, Status> {
        info!("get_nonce called");
        let req = request.into_inner();

        // Log address

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

        // Extract and log the body contents if present
        let tx_hash = if let Some(body) = req.body {
            println!("CreateProgram request details:");
            println!("  Nonce: {}", body.nonce);
            println!("  VK Hash: 0x{}", hex::encode(&body.vk_hash));
            println!("  VK size: {} bytes", body.vk.len());
            println!("  Program URI: {}", body.program_uri);

            let owner = self.state.config.pub_key;
            let mut store = self.state.program_store.store.lock().await;
            // TODO: seconds since UNIX_EPOCH, is this the best format for `created_at` time?
            let created_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            // TODO: who's the owner supposed to be?  Who looks at this?
            let uuid = Uuid::new_v4();
            // TODO: newtype ProofName that is just a prefix of "proof:<uuid>" for easier logging
            // include nice methods for going to/from uuid
            // Can also extend this for all artifact types
            let name = Some(uuid.to_string());

            let program = Program {
                vk_hash: body.vk_hash,
                vk: body.vk,
                program_uri: body.program_uri,
                owner: (*owner).to_vec(),
                created_at,
                name,
            };

            store.insert(uuid, program);

            // Generate a mock transaction hash (in a real implementation, this would be from the blockchain)
            (*B256::random()).to_vec()
        } else {
            // TODO: what situation is this??
            todo!()
        };

        // TODO: verify VK?

        // TODO: who is signing this? Verify signature
        // Log the signature
        println!("Signature: 0x{}", hex::encode(&req.signature));

        // TODO: increment nonce?

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

        // Extract and log the body contents if present

        if let Some(body) = &req.body {
            println!("RequestProof request details:");
            println!("  Nonce: {}", body.nonce);
            println!("  VK Hash: 0x{}", hex::encode(&body.vk_hash));
            println!("  Version: {}", body.version);
            println!("  Mode: {}", body.mode);
            println!("  Strategy: {}", body.strategy);
            println!("  Stdin URI: {}", body.stdin_uri);
            println!("  Deadline: {}", body.deadline);
            println!("  Cycle Limit: {}", body.cycle_limit);
            println!("  Gas Limit: {}", body.gas_limit);
        }

        // Log the signature
        println!("Signature: 0x{}", hex::encode(&req.signature));

        // Generate a mock request ID (this would typically be a unique identifier for the proof request)
        let mut request_id = vec![0u8; 32];
        let uuid = Uuid::new_v4();
        let uuid_bytes = uuid.as_bytes();
        request_id[0..16].copy_from_slice(uuid_bytes);

        // Find an available worker
        let selected_worker = {
            let workers = self.state.workers.read().await;
            workers
                .values()
                .find(|w| w.status == WorkerStatus::Idle)
                .cloned()
        };

        // If we found a worker, dispatch the job
        if let Some(worker) = selected_worker {
            // Spawn a task to send the request to the worker
            // This runs in the background so we don't block the RPC response
            let req_id = request_id.clone();
            tokio::spawn(async move {
                println!(
                    "Sending proof request to contemplant {}:{}",
                    worker.name, worker.id
                );
                // // Create the worker task request
                // let task_request = WorkerTaskRequest {
                //     request_id: hex::encode(&req_id),
                //     // Include all necessary parameters from the original request
                //     vk_hash: hex::encode(&req.body.as_ref().unwrap().vk_hash),
                //     stdin_uri: req.body.as_ref().unwrap().stdin_uri.clone(),
                //     // Any other parameters needed...
                // };
                //
                // // Send the request to the worker
                // match reqwest::Client::new()
                //     .post(format!("{}/task", worker.address))
                //     .json(&task_request)
                //     .send()
                //     .await
                // {
                //     Ok(response) => {
                //         if response.status().is_success() {
                //             println!("Task dispatched to worker: {}", worker.id);
                //             // Update worker status to busy
                //             // ...
                //         } else {
                //             println!("Worker returned error: {}", response.status());
                //             // Handle error, maybe try another worker
                //             // ...
                //         }
                //     }
                //     Err(e) => {
                //         println!("Error sending task to worker: {}", e);
                //         // Handle error, maybe try another worker
                //         // ...
                //     }
                // }
            });

            // TODO: implement a request queue.
        } else {
            println!("No available workers found for the task");
            return Err(Status::internal("no worker available"));
        }

        // Generate a mock transaction hash
        let mut tx_hash = vec![0u8; 32]; // 32-byte transaction hash
        for (i, &byte) in req.signature.iter().take(32).enumerate() {
            tx_hash[i] = byte;
        }

        // Store proof request data for status checks
        let deadline = req.body.as_ref().map(|b| b.deadline).unwrap_or(0);

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
            .insert(request_id.clone(), proof_request_data);

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

        // Look up the request in our database
        let proof_requests = self.state.proof_requests.lock().await;

        match proof_requests.get(&req.request_id) {
            Some(proof_data) => {
                // For testing purposes, let's simulate that the proof is being processed
                // In a real implementation, you would check the actual status from a database

                // Create the response
                let response = GetProofRequestStatusResponse {
                    fulfillment_status: proof_data.fulfillment_status as i32,
                    execution_status: proof_data.execution_status as i32,
                    request_tx_hash: proof_data.tx_hash.clone(),
                    deadline: proof_data.deadline,
                    fulfill_tx_hash: proof_data.fulfill_tx_hash.clone(),
                    proof_uri: proof_data.proof_uri.clone(),
                    public_values_hash: proof_data.public_values_hash.clone(),
                };

                println!("Responding with status:");
                println!(
                    "  Fulfillment Status: {:?}",
                    FulfillmentStatus::from_i32(response.fulfillment_status).unwrap()
                );
                println!(
                    "  Execution Status: {:?}",
                    ExecutionStatus::from_i32(response.execution_status).unwrap()
                );
                println!(
                    "  Request Tx Hash: 0x{}",
                    hex::encode(&response.request_tx_hash)
                );
                println!("  Deadline: {}", response.deadline);

                if let Some(ref fulfill_tx_hash) = response.fulfill_tx_hash {
                    println!("  Fulfill Tx Hash: 0x{}", hex::encode(fulfill_tx_hash));
                }

                if let Some(ref proof_uri) = response.proof_uri {
                    println!("  Proof URI: {}", proof_uri);
                }

                if let Some(ref public_values_hash) = response.public_values_hash {
                    println!(
                        "  Public Values Hash: 0x{}",
                        hex::encode(public_values_hash)
                    );
                }

                Ok(Response::new(response))
            }
            None => {
                // If request ID is not found, return an error
                println!("Request ID not found");
                Err(Status::not_found("Request ID not found"))
            }
        }
    }
}
