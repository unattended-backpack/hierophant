use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::artifact::artifact_store_server::ArtifactStore;
use crate::artifact::{CreateArtifactRequest, CreateArtifactResponse};
use crate::network::prover_network_server::ProverNetwork;
use crate::network::{
    GetNonceRequest, GetNonceResponse, GetProgramRequest, GetProgramResponse, Program,
};

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
}

impl ProverNetworkService {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(MethodRegistry::default())),
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
}

// Our ArtifactStore service implementation
#[derive(Debug)]
pub struct ArtifactStoreService {
    registry: Arc<Mutex<MethodRegistry>>,
    upload_urls: Arc<Mutex<HashSet<String>>>,
}

impl ArtifactStoreService {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(MethodRegistry::default())),
            upload_urls: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    // Get a reference to the upload URLs for the HTTP handler
    pub fn get_upload_urls(&self) -> Arc<Mutex<HashSet<String>>> {
        self.upload_urls.clone()
    }
}

#[tonic::async_trait]
impl ArtifactStore for ArtifactStoreService {
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
        self.upload_urls.lock().unwrap().insert(upload_path);

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
