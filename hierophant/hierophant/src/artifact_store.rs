use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use log::info;
use sp1_sdk::{SP1Stdin, network::proto::artifact::ArtifactType};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ArtifactStore {
    // mapping of artifact upload path to (expected type, uri)
    pub upload_urls: Arc<Mutex<HashMap<String, (ArtifactType, Uuid)>>>,
    // mapping of uri, artifact data
    pub store: Arc<Mutex<HashMap<Uuid, Artifact>>>,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self {
            upload_urls: Arc::new(Mutex::new(HashMap::new())),
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn register_upload_url(
        &self,
        upload_url: String,
        expected_artifact_type: ArtifactType,
        artifact_uri: Uuid,
    ) {
        info!(
            "Registered upload url {upload_url} for artifact {artifact_uri} expecting a {}",
            expected_artifact_type.as_str_name()
        );
        self.upload_urls
            .lock()
            .await
            .insert(upload_url, (expected_artifact_type, artifact_uri));
    }
}

#[derive(Clone, Debug)]
pub struct Artifact {
    pub artifact_type: ArtifactType,
    // serialized representation of the artifact
    pub bytes: Bytes,
}

impl Artifact {
    pub fn new(artifact_type: ArtifactType, bytes: Bytes) -> Self {
        Self {
            artifact_type,
            bytes,
        }
    }
}
