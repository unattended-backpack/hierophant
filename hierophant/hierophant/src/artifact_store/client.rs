use super::{artifact_uri::ArtifactUri, command::ArtifactStoreCommand, store::ArtifactStore};

use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use sp1_sdk::network::proto::artifact::ArtifactType;
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Debug)]
pub struct ArtifactStoreClient {
    command_sender: mpsc::Sender<ArtifactStoreCommand>,
}

impl ArtifactStoreClient {
    pub fn new(
        artifact_directory: &str,
        max_stdin_artifacts_stored: usize,
        max_proof_artifacts_stored: usize,
    ) -> Self {
        let (command_sender, receiver) = mpsc::channel(100);

        let artifact_store = ArtifactStore::new(
            receiver,
            artifact_directory,
            max_stdin_artifacts_stored,
            max_proof_artifacts_stored,
        );

        // start the artifact_store db
        tokio::task::spawn(async move { artifact_store.background_event_loop().await });

        Self { command_sender }
    }

    pub async fn create_artifact(&self, artifact_type: ArtifactType) -> Result<ArtifactUri> {
        let (sender, receiver) = oneshot::channel();
        let command = ArtifactStoreCommand::CreateArtifact {
            artifact_type,
            uri_sender: sender,
        };

        self.command_sender
            .send(command)
            .await
            .context("Send CreateArtifact command")?;

        receiver.await.map_err(|e| anyhow!(e))
    }

    pub async fn save_artifact(
        &self,
        artifact_uri: ArtifactUri,
        artifact_bytes: Bytes,
    ) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let command = ArtifactStoreCommand::SaveArtifact {
            artifact_uri,
            bytes: artifact_bytes,
            result_sender: sender,
        };

        self.command_sender
            .send(command)
            .await
            .context("Send SaveArtifact command")?;

        receiver.await?
    }

    pub async fn get_artifact_bytes(&self, artifact_uri: ArtifactUri) -> Result<Option<Vec<u8>>> {
        let (sender, receiver) = oneshot::channel();
        let command = ArtifactStoreCommand::GetArtifactBytes {
            artifact_uri,
            artifact_sender: sender,
        };

        self.command_sender
            .send(command)
            .await
            .context("Send GetArtifactBytes command")?;

        receiver.await?
    }
}
