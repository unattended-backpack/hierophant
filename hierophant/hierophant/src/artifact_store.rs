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
    upload_urls: Arc<Mutex<HashMap<String, (ArtifactType, Uuid)>>>,
    store: Arc<Mutex<HashMap<Uuid, Artifact>>>,
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

    pub async fn upload(&self, upload_url: String, bytes: Bytes) -> Result<()> {
        let (artifact, uri) = match self.upload_urls.lock().await.get(&upload_url) {
            Some((ArtifactType::Stdin, uri)) => {
                let sp1_stdin: SP1Stdin = bincode::deserialize(bytes.as_ref())
                    .context("deserialize bytes as SP1Stdin")?;
                info!("Uploaded STDIN artifact {uri}");
                (Artifact::Stdin(sp1_stdin), *uri)
            }
            Some((ArtifactType::Program, uri)) => {
                let elf: Vec<u8> = bincode::deserialize(bytes.as_ref())
                    .context("deserialize elf bytes as Vec<u8>")?;
                info!("Uploaded PROGRAM artifact {uri}");
                (Artifact::Program(elf), *uri)
            }
            Some((other_artifact_type, uri)) => {
                return Err(anyhow!(
                    "Unsupported artifact type {} for artifact {uri}",
                    other_artifact_type.as_str_name()
                ));
            }
            None => {
                return Err(anyhow!("Invalid upload URL: {}", upload_url));
            }
        };

        self.store.lock().await.insert(uri, artifact);

        Ok(())
    }
}

#[derive(Clone, Debug)]
enum Artifact {
    Stdin(SP1Stdin),
    // program is an elf
    Program(Vec<u8>),
}
