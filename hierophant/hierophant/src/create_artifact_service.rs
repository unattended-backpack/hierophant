use crate::artifact::create_artifact_server::CreateArtifact;
use crate::artifact::{CreateArtifactRequest, CreateArtifactResponse};
use crate::hierophant_state::{HierophantState, ProofRequestData, WorkerState, WorkerStatus};
use anyhow::anyhow;
use log::info;
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};
use uuid::Uuid;

// Our CreateArtifact service implementation
#[derive(Debug)]
pub struct CreateArtifactService {
    state: Arc<HierophantState>,
}

impl CreateArtifactService {
    pub fn new(state: Arc<HierophantState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl CreateArtifact for CreateArtifactService {
    async fn create_artifact(
        &self,
        request: Request<CreateArtifactRequest>,
    ) -> Result<Response<CreateArtifactResponse>, Status> {
        info!("create_artifact called");
        let req = request.into_inner();

        let artifact_type = match ArtifactType::try_from(req.artifact_type) {
            Ok(t) => t,
            Err(e) => {
                let error_msg = format!("Artifact type {} unknown. {e}", req.artifact_type);
                return Err(Status::invalid_argument(error_msg));
            }
        };

        // Log the artifact type
        info!("Requested artifact type: {}", artifact_type.as_str_name());

        let artifact_uri = Uuid::new_v4();
        let upload_path = format!("/upload/{}", artifact_uri);
        let this_hierophant_ip = self.state.config.this_hierophant_ip.clone();
        let http_port = self.state.config.http_port;
        let artifact_presigned_url =
            format!("http://{this_hierophant_ip}:{http_port}{upload_path}");

        // Register this URL as valid
        self.state
            .upload_urls
            .lock()
            .await
            .insert(upload_path.clone(), (artifact_type, artifact_uri));
        info!(
            "Registered upload url {upload_path} for artifact {artifact_uri} expecting a {}",
            artifact_type.as_str_name()
        );

        // Create the response
        let response = CreateArtifactResponse {
            artifact_uri: artifact_uri.to_string(),
            artifact_presigned_url,
        };

        info!("Responding with artifact URI: {}", response.artifact_uri);
        info!("Presigned URL: {}", response.artifact_presigned_url);

        Ok(Response::new(response))
    }
}
