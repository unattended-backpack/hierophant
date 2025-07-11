use super::artifact_uri::ArtifactUri;

use anyhow::Result;
use axum::body::Bytes;
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::fmt::{self};
use tokio::sync::oneshot;

#[derive(Debug)]
pub(super) enum ArtifactStoreCommand {
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
        };

        write!(f, "{display}",)
    }
}
