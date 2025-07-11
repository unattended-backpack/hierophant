use crate::artifact::artifact_store_server::ArtifactStore;
use crate::artifact::{CreateArtifactRequest, CreateArtifactResponse};
use crate::hierophant_state::HierophantState;
use log::{error, info};
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::sync::Arc;
use tonic::{Request, Response, Status};

// Our CreateArtifact service implementation
pub struct ArtifactStoreService {
    state: Arc<HierophantState>,
}

impl ArtifactStoreService {
    pub fn new(state: Arc<HierophantState>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ArtifactStore for ArtifactStoreService {
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

        let artifact_uri = match self
            .state
            .artifact_store_client
            .create_artifact(artifact_type)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                let error_msg = format!("Error while creating artifact: {e}");
                error!("{error_msg}");
                return Err(Status::unavailable(error_msg));
            }
        };

        let upload_path = format!("/upload/{}", artifact_uri);

        let this_hierophant_ip = self.state.config.this_hierophant_ip.clone();
        let http_port = self.state.config.http_port;
        let artifact_presigned_url =
            format!("http://{this_hierophant_ip}:{http_port}{upload_path}");

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
