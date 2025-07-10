use super::artifact_uri::ArtifactUri;

use anyhow::{Context, Result, anyhow};
use axum::body::Bytes;
use log::{debug, error, info, warn};
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
