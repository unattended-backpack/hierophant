use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use log::{error, info, warn};
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::{
    collections::HashSet,
    fmt::{self},
    fs,
    path::Path,
    str::FromStr,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct ArtifactStoreClient {
    command_sender: mpsc::Sender<ArtifactStoreCommand>,
}

impl ArtifactStoreClient {
    pub fn new(artifact_directory: &str) -> Self {
        let (command_sender, receiver) = mpsc::channel(100);

        let artifact_store = ArtifactStore::new(receiver, artifact_directory);

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

    pub async fn delete_artifact(&self, artifact_uri: ArtifactUri) -> Result<()> {
        let command = ArtifactStoreCommand::DeleteArtifact { artifact_uri };
        self.command_sender
            .send(command)
            .await
            .context("Send DeleteArtifact command")?;

        Ok(())
    }
}

struct ArtifactStore {
    receiver: mpsc::Receiver<ArtifactStoreCommand>,
    // folder where artifacts are saved
    artifact_directory: String,
    // uris that can be uploaded
    upload_uris: HashSet<ArtifactUri>,
}

impl ArtifactStore {
    fn new(receiver: mpsc::Receiver<ArtifactStoreCommand>, artifact_directory: &str) -> Self {
        // Create `artifact_directory` if it doesn't already exist
        let path = Path::new(&artifact_directory);
        if !path.exists() {
            info!(
                "{artifact_directory} directory doesn't exist.  Creating it for saving artifacts."
            );
            fs::create_dir(path)
                .context("Create {artifact_directory} directory")
                .unwrap();
        } else {
            info!("Found {artifact_directory} directory.");
        }

        Self {
            receiver,
            artifact_directory: artifact_directory.to_string(),
            upload_uris: HashSet::new(),
        }
    }

    async fn background_event_loop(mut self) {
        while let Some(command) = self.receiver.recv().await {
            let start = Instant::now();
            let command_string = format!("{}", command);
            match command {
                ArtifactStoreCommand::CreateArtifact {
                    artifact_type,
                    uri_sender,
                } => {
                    let artifact_uri = self.handle_create_artifact(artifact_type);
                    uri_sender.send(artifact_uri).unwrap();
                }
                ArtifactStoreCommand::SaveArtifact {
                    artifact_uri,
                    bytes,
                    result_sender,
                } => {
                    let res = self.handle_save_artifact(artifact_uri, bytes);
                    result_sender.send(res).unwrap();
                }
                ArtifactStoreCommand::GetArtifactBytes {
                    artifact_uri,
                    artifact_sender,
                } => {
                    let res = self.handle_get_artifact_bytes(artifact_uri);
                    artifact_sender.send(res).unwrap();
                }
                ArtifactStoreCommand::DeleteArtifact { artifact_uri } => {
                    let _ = self.handle_delete_artifact(artifact_uri);
                }
            };

            let secs = start.elapsed().as_secs_f64();

            if secs > 0.5 {
                // TODO: remove this or send it to debug.  Just for basic benchmarking
                info!(
                    "Slow execution detected: took {} seconds to process artifact_store command {:?}",
                    secs, command_string
                );
            }
        }
    }

    fn handle_create_artifact(&mut self, artifact_type: ArtifactType) -> ArtifactUri {
        // create uri
        let artifact_uri = ArtifactUri::new(artifact_type);
        // mark this uri as valid for upload
        self.upload_uris.insert(artifact_uri.clone());
        info!("Artifact uri {artifact_uri} listed as valid for upload.");
        artifact_uri
    }

    fn handle_save_artifact(&mut self, artifact_uri: ArtifactUri, bytes: Bytes) -> Result<()> {
        // make sure the uri is listed as a valid upload
        if let None = self.upload_uris.get(&artifact_uri) {
            let error_msg = format!("artifact uri {artifact_uri} is not a registered upload uri");
            error!("{error_msg}");
            return Err(anyhow!("{error_msg}"));
        }

        let artifact_path = artifact_uri.file_path(&self.artifact_directory);
        let path = Path::new(&artifact_path);

        // check to see if this artifact already exists on-disk
        if path.exists() {
            warn!("Artifact {artifact_path} already exists on-disk");
            return Ok(());
        }

        info!(
            "Writing artifact {} to disk.  Num bytes: {}",
            artifact_uri,
            bytes.len()
        );
        // write artifact to disk
        fs::write(path, bytes.to_vec())
            .context(format!("Write artifact to file {artifact_path}"))?;

        info!("Artifact written to {artifact_path}");

        Ok(())
    }

    fn handle_get_artifact_bytes(&mut self, artifact_uri: ArtifactUri) -> Result<Option<Vec<u8>>> {
        let artifact_path = artifact_uri.file_path(&self.artifact_directory);
        let path = Path::new(&artifact_path);
        if path.exists() {
            fs::read(path)
                .context(format!("Loading artifact from file {artifact_path}"))
                .map(|b| Some(b))
        } else {
            Ok(None)
        }
    }

    fn handle_delete_artifact(&mut self, artifact_uri: ArtifactUri) {
        let artifact_path = artifact_uri.file_path(&self.artifact_directory);
        let path = Path::new(&artifact_path);

        match fs::remove_file(path) {
            Ok(_) => {
                info!("Deleted artifact {artifact_uri}");
            }
            Err(e) => {
                warn!("Error trying to delete artifact {artifact_uri}: {e}");
            }
        };
    }
}

#[derive(Debug)]
enum ArtifactStoreCommand {
    CreateArtifact {
        artifact_type: ArtifactType,
        uri_sender: oneshot::Sender<ArtifactUri>,
    },
    SaveArtifact {
        artifact_uri: ArtifactUri,
        bytes: Bytes,
        result_sender: oneshot::Sender<Result<()>>,
    },
    GetArtifactBytes {
        artifact_uri: ArtifactUri,
        artifact_sender: oneshot::Sender<Result<Option<Vec<u8>>>>,
    },
    DeleteArtifact {
        artifact_uri: ArtifactUri,
    },
}

impl fmt::Display for ArtifactStoreCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let display = match self {
            Self::CreateArtifact { artifact_type, .. } => {
                format!("CreateArtifact {}", artifact_type.as_str_name())
            }
            Self::SaveArtifact { artifact_uri, .. } => {
                format!("SaveArtifact {}", artifact_uri)
            }
            Self::GetArtifactBytes { artifact_uri, .. } => {
                format!("GetArtifactBytes {}", artifact_uri)
            }
            Self::DeleteArtifact { artifact_uri, .. } => {
                format!("DeleteArtifact {}", artifact_uri)
            }
        };

        write!(f, "{display}",)
    }
}

// artifact_uri is {artifact_type}-{artifact_uuid}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactUri {
    id: Uuid,
    artifact_type: ArtifactType,
}

impl ArtifactUri {
    fn new(artifact_type: ArtifactType) -> Self {
        let id = Uuid::new_v4();

        Self { id, artifact_type }
    }

    // artifact will be written to {artifact_directory}/{artifact_uri}
    fn file_path(&self, artifact_directory: &str) -> String {
        format!("{}/{}", artifact_directory, self)
    }
}

// {artifact_type}-{artifact_uuid}
impl fmt::Display for ArtifactUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}_{}", self.artifact_type.as_str_name(), self.id)
    }
}

impl fmt::Display for ParseArtifactUriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to parse ArtifactUri")
    }
}

#[derive(Debug, Clone)]
pub struct ParseArtifactUriError;

impl std::error::Error for ParseArtifactUriError {}

// Implement FromStr for ArtifactUri
impl FromStr for ArtifactUri {
    type Err = ParseArtifactUriError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split the string by '_'
        let parts: Vec<&str> = s.split('_').collect();

        // Check if we have exactly 2 parts
        if parts.len() != 2 {
            return Err(ParseArtifactUriError);
        }

        // Parse the artifact type
        let artifact_type =
            ArtifactType::from_str_name(parts[0]).ok_or_else(|| ParseArtifactUriError)?;

        // Parse the UUID
        let id = Uuid::parse_str(parts[1]).map_err(|_| ParseArtifactUriError)?;

        Ok(ArtifactUri { id, artifact_type })
    }
}

impl<'de> Deserialize<'de> for ArtifactUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ArtifactUriVisitor;

        impl<'de> Visitor<'de> for ArtifactUriVisitor {
            type Value = ArtifactUri;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string in the format 'artifact_type-uuid'")
            }

            fn visit_str<E>(self, value: &str) -> Result<ArtifactUri, E>
            where
                E: de::Error,
            {
                ArtifactUri::from_str(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(ArtifactUriVisitor)
    }
}
