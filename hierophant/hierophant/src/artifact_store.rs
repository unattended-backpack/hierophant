use alloy_primitives::B256;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::artifact::{CreateArtifactRequest, CreateArtifactResponse};
use sp1_sdk::{
    CpuProver, CudaProver, Prover, ProverClient, network::proto::artifact::ArtifactType, utils,
};

const ARTIFACT_URI_PREFIX: &str = "hierophant://artifacts/";

#[derive(Debug, Clone)]
pub struct Artifact {
    id: Uuid,
    uri: String,
    artifact_type: ArtifactType,
    data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct ArtifactStoreClient {
    artifact_store: Arc<Mutex<HashMap<Uuid, Artifact>>>,
    upload_urls: Arc<Mutex<HashSet<String>>>,
}

impl ArtifactStoreClient {
    pub fn new() -> Self {
        ArtifactStoreClient {
            artifact_store: Arc::new(Mutex::new(HashMap::new())),
            upload_urls: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    // pub struct CreateArtifactRequest {
    //     /// The signature of the user on a pre-defined message. Used for authentication
    //     #[prost(bytes = "vec", tag = "1")]
    //     pub signature: ::prost::alloc::vec::Vec<u8>,
    //     /// The type of artifact to create
    //     #[prost(enumeration = "ArtifactType", tag = "2")]
    //     pub artifact_type: i32,
    // }

    pub fn create_artifact(request: CreateArtifactRequest) -> CreateArtifactResponse {
        let id = Uuid::new_v4();
        let uri = format!("{ARTIFACT_URI_PREFIX}{id}");

        // Use a local URL for uploads
        let upload_path = format!("/upload/{}", id);
        let artifact_presigned_url = format!("http://0.0.0.0:9010{}", upload_path);

        todo!()
    }

    // pub struct CreateArtifactResponse {
    //     /// The unique resource identifier of the artifact
    //     #[prost(string, tag = "1")]
    //     pub artifact_uri: ::prost::alloc::string::String,
    //     /// The presigned url to upload the artifact
    //     #[prost(string, tag = "2")]
    //     pub artifact_presigned_url: ::prost::alloc::string::String,
    // }
}
