use serde::{Deserialize, Serialize};

// Reports which openvm-sdk version this hierophant verifies against, in the
// same loose shape as the Bonsai version endpoint.
#[derive(Serialize)]
pub struct VersionResponse {
    pub openvm: Vec<String>,
}

#[derive(Serialize)]
pub struct ProgramUploadResponse {
    pub url: String,
}

#[derive(Serialize)]
pub struct InputUploadResponse {
    pub uuid: String,
    pub url: String,
}

#[derive(Deserialize)]
pub struct ProofCreateRequest {
    // program_id previously uploaded via PUT /openvm/programs/{id}
    pub program: String,
    // input uuid previously uploaded via PUT /openvm/inputs/{uuid}
    pub input: String,
    // app | stark | evm; defaults to app
    #[serde(default)]
    pub proof_mode: Option<String>,
    // Optional `openvm.toml` app-config contents (the `[app_vm_config.*]`
    // tables). Omit for guests built against the SDK's standard config.
    #[serde(default)]
    pub app_config_toml: Option<String>,
}

#[derive(Serialize)]
pub struct ProofCreateResponse {
    pub uuid: String,
}

#[derive(Serialize)]
pub struct ProofStatusResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}
