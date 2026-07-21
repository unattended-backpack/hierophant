use super::state::OpenVmJob;
use super::types::*;
use crate::hierophant_state::HierophantState;

use alloy_primitives::B256;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use log::{debug, error, info, warn};
use network_lib::OpenVmProofMode;
use sha2::{Digest, Sha256};
use sp1_sdk::network::proto::network::{ExecutionStatus, FulfillmentStatus};
use std::sync::Arc;
use uuid::Uuid;

pub fn openvm_routes() -> Router<Arc<HierophantState>> {
    Router::new()
        .route("/version", get(handle_version))
        .route("/programs/upload/:program_id", get(handle_program_upload_url))
        .route("/programs/:program_id", put(handle_program_put))
        .route("/inputs/upload", get(handle_input_upload_url))
        .route("/inputs/:input_id", put(handle_input_put))
        .route("/proofs/create", post(handle_proof_create))
        .route("/proofs/status/:proof_id", get(handle_proof_status))
        // Download the verified proof produced by a job. The URL is what
        // handle_proof_status hands back in ProofStatusResponse.proof_url.
        .route("/proofs/:proof_id/download", get(handle_proof_download))
}

async fn handle_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        openvm: vec![super::verify::OPENVM_VERSION.into()],
    })
}

fn presigned_url(state: &HierophantState, suffix: &str) -> String {
    format!(
        "http://{}:{}/openvm{}",
        state.config.this_hierophant_ip, state.config.http_port, suffix
    )
}

// program_id convention: lowercase hex sha256 of the raw ELF bytes. Cheap for
// both sides to compute, and (unlike RISC Zero's image_id) OpenVM has no
// keygen-free native program digest we could check at upload time; the true
// program commitment is enforced later, at proof verification.
fn normalize_program_id(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    s.to_ascii_lowercase()
}

async fn handle_program_upload_url(
    State(state): State<Arc<HierophantState>>,
    Path(program_id): Path<String>,
) -> Result<Json<ProgramUploadResponse>, StatusCode> {
    let program_id = normalize_program_id(&program_id);
    let url = presigned_url(&state, &format!("/programs/{program_id}"));
    Ok(Json(ProgramUploadResponse { url }))
}

async fn handle_program_put(
    State(state): State<Arc<HierophantState>>,
    Path(program_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    let program_id = normalize_program_id(&program_id);
    info!("OpenVM program upload {program_id} ({} bytes)", body.len());

    // Defense-in-depth parity with the Bonsai image upload: check the claimed
    // id against the digest we compute. A mismatch means client confusion; the
    // proof-level program commitment check at verification time is what
    // actually protects against a wrong program.
    let computed = format!("{:x}", Sha256::digest(&body));
    if computed != program_id {
        warn!(
            "OpenVM program upload: provided program_id {program_id} doesn't match computed sha256 {computed}; accepting anyway and logging"
        );
    }

    state
        .openvm
        .inner
        .lock()
        .await
        .programs
        .insert(program_id, body.to_vec());
    Ok(StatusCode::OK)
}

async fn handle_input_upload_url(
    State(state): State<Arc<HierophantState>>,
) -> Json<InputUploadResponse> {
    let uuid = Uuid::new_v4().to_string();
    let url = presigned_url(&state, &format!("/inputs/{uuid}"));
    Json(InputUploadResponse { uuid, url })
}

async fn handle_input_put(
    State(state): State<Arc<HierophantState>>,
    Path(input_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // The body is a bincode-serialized Vec<Vec<u8>>: one entry per StdIn hint
    // stream, in guest read order. Decode here so a malformed upload fails at
    // PUT time with a clear error instead of surfacing as a worker-side prove
    // failure later.
    let streams: Vec<Vec<u8>> = bincode::deserialize(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("input body must be a bincode-serialized Vec<Vec<u8>> of stdin streams: {e}"),
        )
    })?;
    info!(
        "OpenVM input upload {input_id} ({} streams, {} bytes)",
        streams.len(),
        body.len()
    );
    state
        .openvm
        .inner
        .lock()
        .await
        .inputs
        .insert(input_id, streams);
    Ok(StatusCode::OK)
}

async fn handle_proof_create(
    State(state): State<Arc<HierophantState>>,
    Json(req): Json<ProofCreateRequest>,
) -> Result<Json<ProofCreateResponse>, (StatusCode, String)> {
    let program_id = normalize_program_id(&req.program);
    let input_id = req.input.clone();

    let (elf, input) = {
        let inner = state.openvm.inner.lock().await;
        let elf = inner.programs.get(&program_id).cloned().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("unknown program_id {program_id}"),
            )
        })?;
        let input = inner.inputs.get(&input_id).cloned().ok_or_else(|| {
            (StatusCode::BAD_REQUEST, format!("unknown input_id {input_id}"))
        })?;
        (elf, input)
    };

    let mode = parse_proof_mode(req.proof_mode.as_deref())
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let request_id = B256::random();
    let proof_uuid = Uuid::new_v4().to_string();

    state.openvm.inner.lock().await.jobs.insert(
        proof_uuid.clone(),
        OpenVmJob {
            request_id,
            program_id: program_id.clone(),
            app_config_toml: req.app_config_toml.clone(),
            mode,
            terminal_error: None,
        },
    );

    info!(
        "OpenVM proof job {proof_uuid} created (request_id {request_id}, program_id {program_id}, input_id {input_id}, mode {})",
        mode.as_str()
    );

    if let Err(e) = state
        .proof_router
        .route_openvm_proof(request_id, elf, req.app_config_toml, input, mode)
        .await
    {
        error!("Error routing OpenVM job {proof_uuid}: {e}");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("route proof: {e}"),
        ));
    }

    Ok(Json(ProofCreateResponse { uuid: proof_uuid }))
}

async fn handle_proof_status(
    State(state): State<Arc<HierophantState>>,
    Path(proof_id): Path<String>,
) -> Result<Json<ProofStatusResponse>, (StatusCode, String)> {
    let (request_id, program_id, app_config_toml, mode, terminal_error) = {
        let inner = state.openvm.inner.lock().await;
        match inner.jobs.get(&proof_id) {
            Some(j) => (
                j.request_id,
                j.program_id.clone(),
                j.app_config_toml.clone(),
                j.mode,
                j.terminal_error.clone(),
            ),
            None => {
                return Err((StatusCode::NOT_FOUND, format!("unknown proof {proof_id}")));
            }
        }
    };

    if let Some(err) = terminal_error {
        return Ok(Json(ProofStatusResponse {
            status: "FAILED".into(),
            proof_url: None,
            error_msg: Some(err),
            state: None,
        }));
    }

    // Already have a verified proof stashed -> job is SUCCEEDED.
    if state.openvm.inner.lock().await.proofs.contains_key(&proof_id) {
        let url = presigned_url(&state, &format!("/proofs/{proof_id}/download"));
        return Ok(Json(ProofStatusResponse {
            status: "SUCCEEDED".into(),
            proof_url: Some(url),
            error_msg: None,
            state: None,
        }));
    }

    // Poll the worker registry for status.
    let proof_status = match state.proof_router.get_proof_status(request_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("get_proof_status failed for OpenVM job {proof_id}: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("status poll: {e}")));
        }
    };

    let fulfilled: i32 = FulfillmentStatus::Fulfilled.into();
    let unfulfillable: i32 = FulfillmentStatus::Unfulfillable.into();

    if proof_status.fulfillment_status == unfulfillable {
        // Either the proof was truly unexecutable or the assigned worker was
        // lost. Record a terminal failure on the job so subsequent polls are
        // cheap.
        let err = "proof reported unfulfillable by worker registry".to_string();
        if let Some(j) = state.openvm.inner.lock().await.jobs.get_mut(&proof_id) {
            j.terminal_error = Some(err.clone());
        }
        return Ok(Json(ProofStatusResponse {
            status: "FAILED".into(),
            proof_url: None,
            error_msg: Some(err),
            state: None,
        }));
    }

    if proof_status.fulfillment_status != fulfilled || proof_status.proof.is_empty() {
        return Ok(Json(ProofStatusResponse {
            status: "RUNNING".into(),
            proof_url: None,
            error_msg: None,
            state: Some(format!(
                "execution_status={}",
                ExecutionStatus::try_from(proof_status.execution_status)
                    .map(|e| e.as_str_name())
                    .unwrap_or("UNSPECIFIED")
            )),
        }));
    }

    // Proof bytes in hand: re-derive keys/commitments from the uploaded ELF +
    // config and verify. A broken or wrong-program proof is a worker-side bug
    // or a malicious worker; strike+drop the worker to force reassignment
    // (parity with the SP1 and Bonsai paths). Verification includes keygen
    // (cached per config), which is CPU-heavy, so run it on a blocking thread.
    let elf = match state
        .openvm
        .inner
        .lock()
        .await
        .programs
        .get(&program_id)
        .cloned()
    {
        Some(elf) => elf,
        None => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("program for OpenVM job {proof_id} missing at verify time"),
            ));
        }
    };

    let proof_bytes = proof_status.proof;
    let proof_bytes_for_verify = proof_bytes.clone();
    let verify_res = tokio::task::spawn_blocking(move || {
        super::verify::verify_openvm_proof(&elf, app_config_toml.as_deref(), mode, &proof_bytes_for_verify)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("verification task join error: {e}"),
        )
    })?;

    if let Err(e) = verify_res {
        warn!(
            "OpenVM job {proof_id}: proof verification failed ({e}). Dropping worker and failing job."
        );
        state
            .proof_router
            .worker_registry_client
            .drop_worker_of_request(request_id)
            .await;
        let err = format!("worker returned invalid proof: {e}");
        if let Some(j) = state.openvm.inner.lock().await.jobs.get_mut(&proof_id) {
            j.terminal_error = Some(err.clone());
        }
        return Ok(Json(ProofStatusResponse {
            status: "FAILED".into(),
            proof_url: None,
            error_msg: Some(err),
            state: None,
        }));
    }

    info!(
        "Verified OpenVM proof for job {proof_id} (mode {})",
        mode.as_str()
    );

    if let Err(e) = state
        .proof_router
        .worker_registry_client
        .proof_complete(request_id)
        .await
    {
        error!("proof_complete command failed for OpenVM job {proof_id}: {e}");
    }

    {
        let mut inner = state.openvm.inner.lock().await;
        inner.proofs.insert(proof_id.clone(), proof_bytes);
    }

    let url = presigned_url(&state, &format!("/proofs/{proof_id}/download"));
    Ok(Json(ProofStatusResponse {
        status: "SUCCEEDED".into(),
        proof_url: Some(url),
        error_msg: None,
        state: None,
    }))
}

async fn handle_proof_download(
    State(state): State<Arc<HierophantState>>,
    Path(proof_id): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    match state.openvm.inner.lock().await.proofs.get(&proof_id) {
        Some(bytes) => {
            debug!(
                "Serving OpenVM proof for job {proof_id} ({} bytes)",
                bytes.len()
            );
            Ok(bytes.clone())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

fn parse_proof_mode(raw: Option<&str>) -> Result<OpenVmProofMode, String> {
    match raw.map(|s| s.to_ascii_lowercase()) {
        None => Ok(OpenVmProofMode::App),
        Some(ref s) if s == "app" => Ok(OpenVmProofMode::App),
        Some(ref s) if s == "stark" => Ok(OpenVmProofMode::Stark),
        Some(ref s) if s == "evm" => Ok(OpenVmProofMode::Evm),
        Some(other) => Err(format!(
            "unsupported proof_mode '{other}' (expected app|stark|evm)"
        )),
    }
}
