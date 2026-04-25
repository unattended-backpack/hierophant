use serde::{Deserialize, Serialize};

// Bonsai's `GET /version` returns a tagged version string; the SDK parses it
// loosely so we only need to surface a `risc0_zkvm` field.
#[derive(Serialize)]
pub struct VersionResponse {
    pub risc0_zkvm: Vec<String>,
}

#[derive(Serialize)]
pub struct ImageUploadResponse {
    pub url: String,
}

#[derive(Serialize)]
pub struct InputUploadResponse {
    pub uuid: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct ReceiptUploadResponse {
    pub uuid: String,
    pub url: String,
}

#[derive(Deserialize)]
pub struct SessionCreateRequest {
    pub img: String,
    pub input: String,
    #[serde(default)]
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub execute_only: bool,
    // Accepted but not enforced server-side yet; retained so bonsai-sdk
    // clients that send it don't get rejected at deserialization.
    #[serde(default)]
    #[allow(dead_code)]
    pub exec_cycle_limit: Option<u64>,
    #[serde(default)]
    pub proof_mode: Option<String>,
}

#[derive(Serialize)]
pub struct SessionCreateResponse {
    pub uuid: String,
}

#[derive(Serialize)]
pub struct SessionStatusResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_time: Option<f64>,
}

#[derive(Deserialize)]
pub struct SnarkCreateRequest {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct SnarkCreateResponse {
    pub uuid: String,
}

// bonsai-sdk's `SnarkStatusRes` expects `output` as an Option<String> that
// points to a URL where the bincode-encoded risc0::Receipt can be downloaded
// (NOT a structured json object). See bonsai-sdk-1.4.2/src/lib.rs:294; the
// doc comment there is literal: "Url to download the snark (receipt
// risc0::Receipt bincode encoded)".
#[derive(Serialize)]
pub struct SnarkStatusResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}
