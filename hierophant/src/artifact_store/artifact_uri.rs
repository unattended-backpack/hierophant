use anyhow::Result;
use serde::{
    Deserialize, Deserializer,
    de::{self, Visitor},
};
use sp1_sdk::network::proto::artifact::ArtifactType;
use std::{
    fmt::{self},
    str::FromStr,
};
use uuid::Uuid;

// artifact_uri is {artifact_type}-{artifact_uuid}
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtifactUri {
    pub id: Uuid,
    pub artifact_type: ArtifactType,
}

impl ArtifactUri {
    pub fn new(artifact_type: ArtifactType) -> Self {
        let id = Uuid::new_v4();

        Self { id, artifact_type }
    }

    // artifact will be written to {artifact_directory}/{artifact_uri}
    pub fn file_path(&self, artifact_directory: &str) -> String {
        format!("{artifact_directory}/{self}")
    }
}

impl Default for ArtifactUri {
    fn default() -> Self {
        let id = Uuid::nil();
        let artifact_type = ArtifactType::UnspecifiedArtifactType;
        Self { id, artifact_type }
    }
}

// artifact uri format:
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
        let artifact_type = ArtifactType::from_str_name(parts[0]).ok_or(ParseArtifactUriError)?;

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

        impl Visitor<'_> for ArtifactUriVisitor {
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
