use crate::artifact::create_artifact_server::CreateArtifact;
use crate::artifact::{CreateArtifactRequest, CreateArtifactResponse};
use crate::hierophant_state::{HierophantState, ProofRequestData, WorkerState, WorkerStatus};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    CreateProgramRequest, CreateProgramResponse, CreateProgramResponseBody, ExecutionStatus,
    FulfillmentStatus, GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse,
    GetProofRequestStatusRequest, GetProofRequestStatusResponse, Program, RequestProofRequest,
    RequestProofResponse, RequestProofResponseBody,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::{Request, Response, Status};
use uuid::Uuid;

// TODO: delete all method registry mentions.
// Structure to track method names
#[derive(Debug, Default)]
pub struct MethodRegistry {
    pub methods: HashSet<String>,
}

// Helper to log method calls
pub fn log_method_call(
    method_name: &str,
    request: &impl prost::Message,
    registry: &mut MethodRegistry,
) {
    println!("\n=== Method Call: {} ===", method_name);

    // Register this method
    registry.methods.insert(method_name.to_string());

    // Log all methods seen so far
    println!("Methods seen so far: {:?}", registry.methods);

    // Try to get bytes for debug
    let bytes = request.encode_to_vec();
    println!("Request size: {} bytes", bytes.len());

    // Print as hex dump for easier analysis
    if !bytes.is_empty() {
        println!("Request Hex dump:");
        for (i, chunk) in bytes.chunks(16).enumerate() {
            let hex_line = hex::encode(chunk);
            let mut ascii_line = String::new();

            for &byte in chunk {
                if byte >= 32 && byte <= 126 {
                    ascii_line.push(byte as char);
                } else {
                    ascii_line.push('.');
                }
            }

            println!("{:08x}: {:48} | {}", i * 16, hex_line, ascii_line);
        }
    }
}

// Our ProverNetwork service implementation
#[derive(Debug)]
pub struct ProverNetworkService {
    registry: Arc<Mutex<MethodRegistry>>,
    state: Arc<HierophantState>,
}

impl ProverNetworkService {
    pub fn new(state: Arc<HierophantState>) -> Self {
        Self {
            registry: Arc::new(Mutex::new(MethodRegistry::default())),
            state,
        }
    }
}

#[tonic::async_trait]
impl ProverNetwork for ProverNetworkService {
    async fn get_program(
        &self,
        request: Request<GetProgramRequest>,
    ) -> Result<Response<GetProgramResponse>, Status> {
        let req = request.into_inner();

        // Log method call
        log_method_call("GetProgram", &req, &mut self.registry.lock().unwrap());

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
        let req = request.into_inner();

        // Log method call
        log_method_call("GetNonce", &req, &mut self.registry.lock().unwrap());

        // Log address
        println!(
            "Requested nonce for address: 0x{}",
            hex::encode(&req.address)
        );

        // Generate a mock nonce (in a real implementation, this would come from a database)
        // Using a simple deterministic value based on the address for consistency
        // TODO: make this a real nonce.
        let mock_nonce = req
            .address
            .iter()
            .fold(0u64, |acc, &byte| acc.wrapping_add(byte as u64))
            % 1000;

        // Create the response
        let response = GetNonceResponse { nonce: mock_nonce };

        println!("Responding with nonce: {}", response.nonce);

        Ok(Response::new(response))
    }

    async fn create_program(
        &self,
        request: Request<CreateProgramRequest>,
    ) -> Result<Response<CreateProgramResponse>, Status> {
        let req = request.into_inner();

        // Log method call
        log_method_call("CreateProgram", &req, &mut self.registry.lock().unwrap());

        // Extract and log the body contents if present
        if let Some(body) = &req.body {
            println!("CreateProgram request details:");
            println!("  Nonce: {}", body.nonce);
            println!("  VK Hash: 0x{}", hex::encode(&body.vk_hash));
            println!("  VK size: {} bytes", body.vk.len());
            println!("  Program URI: {}", body.program_uri);
        }

        // Log the signature
        println!("Signature: 0x{}", hex::encode(&req.signature));

        // Generate a mock transaction hash (in a real implementation, this would be from the blockchain)
        let mut tx_hash = vec![0u8; 32]; // 32-byte transaction hash
        for (i, &byte) in req.signature.iter().take(32).enumerate() {
            tx_hash[i] = byte;
        }

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
        let req = request.into_inner();

        // Log method call
        log_method_call("RequestProof", &req, &mut self.registry.lock().unwrap());

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
        let req = request.into_inner();

        // Log method call
        log_method_call(
            "GetProofRequestStatus",
            &req,
            &mut self.registry.lock().unwrap(),
        );

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

// Our CreateArtifact service implementation
#[derive(Debug)]
pub struct CreateArtifactService {
    registry: Arc<Mutex<MethodRegistry>>,
    state: Arc<HierophantState>,
}

impl CreateArtifactService {
    pub fn new(state: Arc<HierophantState>) -> Self {
        Self {
            registry: Arc::new(Mutex::new(MethodRegistry::default())),
            state,
        }
    }
}

#[tonic::async_trait]
impl CreateArtifact for CreateArtifactService {
    async fn create_artifact(
        &self,
        request: Request<CreateArtifactRequest>,
    ) -> Result<Response<CreateArtifactResponse>, Status> {
        let req = request.into_inner();

        // Log method call
        log_method_call("CreateArtifact", &req, &mut self.registry.lock().unwrap());

        // Log the artifact type
        println!("Requested artifact type: {}", req.artifact_type);

        // Create mock response data
        let artifact_id = Uuid::new_v4().to_string();
        let artifact_uri = format!("hierophant://artifacts/{}", artifact_id);

        // Use a local URL for uploads
        let upload_path = format!("/upload/{}", artifact_id);
        let artifact_presigned_url = format!("http://0.0.0.0:9010{}", upload_path);

        // Register this URL as valid
        self.state.upload_urls.lock().await.insert(upload_path);

        // Create the response
        let response = CreateArtifactResponse {
            artifact_uri,
            artifact_presigned_url,
        };

        println!("Responding with artifact URI: {}", response.artifact_uri);
        println!("Presigned URL: {}", response.artifact_presigned_url);

        Ok(Response::new(response))
    }
}
