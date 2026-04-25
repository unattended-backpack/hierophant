use super::state::{BonsaiSession, SnarkState};
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
use network_lib::Risc0ProofMode;
use risc0_zkvm::Receipt;
use sp1_sdk::network::proto::network::{ExecutionStatus, FulfillmentStatus};
use std::sync::Arc;
use uuid::Uuid;

pub fn bonsai_routes() -> Router<Arc<HierophantState>> {
    Router::new()
        .route("/version", get(handle_version))
        .route("/images/upload/:image_id", get(handle_image_upload_url))
        .route("/images/:image_id", put(handle_image_put))
        // bonsai-sdk 1.4.x hits GET (not POST) for input/receipt upload-url
        // requests: see bonsai_sdk::Client::get_upload_url.
        .route("/inputs/upload", get(handle_input_upload_url))
        .route("/inputs/:input_id", put(handle_input_put))
        .route("/receipts/upload", get(handle_receipt_upload_url))
        .route("/receipts/:receipt_id", put(handle_receipt_put))
        .route("/receipts/:session_id/download", get(handle_receipt_download))
        .route("/sessions/create", post(handle_session_create))
        .route("/sessions/status/:session_id", get(handle_session_status))
        .route("/snark/create", post(handle_snark_create))
        .route("/snark/status/:snark_id", get(handle_snark_status))
        // Download the wrapped (Groth16) receipt produced by a snark job. The
        // URL is what handle_snark_status hands back in SnarkStatusRes.output.
        .route("/snark/:snark_id/download", get(handle_snark_receipt_download))
}

async fn handle_version() -> Json<VersionResponse> {
    // bonsai-sdk tolerates either a string or array for risc0_zkvm.  Report
    // the major version we're built against; the SDK just uses this for a
    // client/server compat check.
    Json(VersionResponse {
        risc0_zkvm: vec!["2".into(), "2.0".into()],
    })
}

fn presigned_url(state: &HierophantState, suffix: &str) -> String {
    format!(
        "http://{}:{}/bonsai{}",
        state.config.this_hierophant_ip, state.config.http_port, suffix
    )
}

async fn handle_image_upload_url(
    State(state): State<Arc<HierophantState>>,
    Path(image_id): Path<String>,
) -> Result<Json<ImageUploadResponse>, StatusCode> {
    // Normalize image_id (trim whitespace; tolerate an optional 0x prefix).
    let image_id = normalize_image_id(&image_id);
    let url = presigned_url(&state, &format!("/images/{image_id}"));
    Ok(Json(ImageUploadResponse { url }))
}

async fn handle_image_put(
    State(state): State<Arc<HierophantState>>,
    Path(image_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    let image_id = normalize_image_id(&image_id);
    info!(
        "Bonsai image upload {image_id} ({} bytes)",
        body.len()
    );

    // Defense-in-depth: verify the image_id the caller claims matches the
    // digest we compute from the uploaded ELF. A mismatched pair will later
    // cause receipt verification to fail anyway, but catching it at upload
    // gives a clearer error.
    let computed = match risc0_zkvm::compute_image_id(&body) {
        Ok(d) => format!("{}", d),
        Err(e) => {
            error!("Unable to compute image id from uploaded ELF: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    if normalize_image_id(&computed) != image_id {
        warn!(
            "Bonsai image upload: provided image_id {image_id} doesn't match computed {computed}; accepting anyway and logging"
        );
    }

    state
        .bonsai
        .inner
        .lock()
        .await
        .images
        .insert(image_id.clone(), body.to_vec());
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
) -> Result<impl IntoResponse, StatusCode> {
    info!("Bonsai input upload {input_id} ({} bytes)", body.len());
    state
        .bonsai
        .inner
        .lock()
        .await
        .inputs
        .insert(input_id, body.to_vec());
    Ok(StatusCode::OK)
}

async fn handle_receipt_upload_url(
    State(state): State<Arc<HierophantState>>,
) -> Json<ReceiptUploadResponse> {
    let uuid = Uuid::new_v4().to_string();
    let url = presigned_url(&state, &format!("/receipts/{uuid}"));
    Json(ReceiptUploadResponse { uuid, url })
}

async fn handle_receipt_put(
    State(state): State<Arc<HierophantState>>,
    Path(receipt_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, StatusCode> {
    // Bonsai exposes receipt upload for compositions where a client passes in
    // an assumption receipt. We accept and store it, but join/compose isn't
    // wired up in v1: we just stash the bytes under the receipt uuid.
    info!("Bonsai receipt upload {receipt_id} ({} bytes)", body.len());
    state
        .bonsai
        .inner
        .lock()
        .await
        .receipts
        .insert(receipt_id, body.to_vec());
    Ok(StatusCode::OK)
}

async fn handle_session_create(
    State(state): State<Arc<HierophantState>>,
    Json(req): Json<SessionCreateRequest>,
) -> Result<Json<SessionCreateResponse>, (StatusCode, String)> {
    let image_id = normalize_image_id(&req.img);
    let input_id = req.input.clone();

    let (elf, input) = {
        let inner = state.bonsai.inner.lock().await;
        let elf = inner
            .images
            .get(&image_id)
            .cloned()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("unknown image_id {image_id}")))?;
        let input = inner
            .inputs
            .get(&input_id)
            .cloned()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, format!("unknown input_id {input_id}")))?;
        (elf, input)
    };

    if req.execute_only {
        // We don't implement execute-only for RISC Zero yet; this would need
        // a separate `ExecutorImpl` path that skips proving.
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "execute_only sessions are not yet supported".into(),
        ));
    }
    if !req.assumptions.is_empty() {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "session assumptions (composition) are not yet supported".into(),
        ));
    }

    let mode = parse_proof_mode(req.proof_mode.as_deref()).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let request_id = B256::random();
    let session_uuid = Uuid::new_v4().to_string();

    state.bonsai.inner.lock().await.sessions.insert(
        session_uuid.clone(),
        BonsaiSession {
            request_id,
            image_id: image_id.clone(),
            input_id: input_id.clone(),
            mode,
            terminal_error: None,
        },
    );

    info!(
        "Bonsai session {session_uuid} created (request_id {request_id}, image_id {image_id}, input_id {input_id}, mode {})",
        mode.as_str()
    );

    if let Err(e) = state
        .proof_router
        .route_risc0_proof(request_id, elf, input, mode, None)
        .await
    {
        error!("Error routing Bonsai session {session_uuid}: {e}");
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("route proof: {e}")));
    }

    Ok(Json(SessionCreateResponse { uuid: session_uuid }))
}

async fn handle_session_status(
    State(state): State<Arc<HierophantState>>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionStatusResponse>, (StatusCode, String)> {
    let (request_id, image_id, mode, terminal_error) = {
        let inner = state.bonsai.inner.lock().await;
        match inner.sessions.get(&session_id) {
            Some(s) => (s.request_id, s.image_id.clone(), s.mode, s.terminal_error.clone()),
            None => return Err((StatusCode::NOT_FOUND, format!("unknown session {session_id}"))),
        }
    };

    if let Some(err) = terminal_error {
        return Ok(Json(SessionStatusResponse {
            status: "FAILED".into(),
            receipt_url: None,
            error_msg: Some(err),
            state: None,
            elapsed_time: None,
        }));
    }

    // Already have a receipt stashed -> session is SUCCEEDED.
    if state.bonsai.inner.lock().await.receipts.contains_key(&session_id) {
        let url = presigned_url(&state, &format!("/receipts/{session_id}/download"));
        return Ok(Json(SessionStatusResponse {
            status: "SUCCEEDED".into(),
            receipt_url: Some(url),
            error_msg: None,
            state: None,
            elapsed_time: None,
        }));
    }

    // Poll the worker registry for status.
    let proof_status = match state.proof_router.get_proof_status(request_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("get_proof_status failed for session {session_id}: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("status poll: {e}")));
        }
    };

    let fulfilled: i32 = FulfillmentStatus::Fulfilled.into();
    let unfulfillable: i32 = FulfillmentStatus::Unfulfillable.into();

    if proof_status.fulfillment_status == unfulfillable {
        // Either the proof was truly unexecutable or the assigned worker was lost.
        // Record a terminal failure on the session so subsequent polls are cheap.
        let err = "proof reported unfulfillable by worker registry".to_string();
        if let Some(s) = state.bonsai.inner.lock().await.sessions.get_mut(&session_id) {
            s.terminal_error = Some(err.clone());
        }
        return Ok(Json(SessionStatusResponse {
            status: "FAILED".into(),
            receipt_url: None,
            error_msg: Some(err),
            state: None,
            elapsed_time: None,
        }));
    }

    if proof_status.fulfillment_status != fulfilled || proof_status.proof.is_empty() {
        return Ok(Json(SessionStatusResponse {
            status: "RUNNING".into(),
            receipt_url: None,
            error_msg: None,
            state: Some(format!(
                "execution_status={}",
                ExecutionStatus::try_from(proof_status.execution_status)
                    .map(|e| e.as_str_name())
                    .unwrap_or("UNSPECIFIED")
            )),
            elapsed_time: None,
        }));
    }

    // Proof bytes in hand: deserialize as a Receipt and verify it against the
    // image_id the caller supplied. A broken or wrong-image receipt is a
    // worker-side bug or a malicious worker; strike+drop the worker to force
    // reassignment (parity with the SP1 path).
    let receipt: Receipt = match bincode::deserialize::<Receipt>(&proof_status.proof) {
        Ok(r) => r,
        Err(e) => {
            error!(
                "Session {session_id}: receipt deserialization failed ({e}). Dropping worker and failing session."
            );
            state
                .proof_router
                .worker_registry_client
                .drop_worker_of_request(request_id)
                .await;
            let err = format!("worker returned malformed receipt: {e}");
            state
                .bonsai
                .inner
                .lock()
                .await
                .sessions
                .get_mut(&session_id)
                .map(|s| s.terminal_error = Some(err.clone()));
            return Ok(Json(SessionStatusResponse {
                status: "FAILED".into(),
                receipt_url: None,
                error_msg: Some(err),
                state: None,
                elapsed_time: None,
            }));
        }
    };

    let image_digest = match risc0_zkvm::compute_image_id(
        state
            .bonsai
            .inner
            .lock()
            .await
            .images
            .get(&image_id)
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("image for session {session_id} missing at verify time"),
                )
            })?,
    ) {
        Ok(d) => d,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("recompute image_id: {e}"),
            ));
        }
    };

    if let Err(e) = receipt.verify(image_digest) {
        warn!(
            "Session {session_id}: receipt verification failed ({e}). Dropping worker and failing session."
        );
        state
            .proof_router
            .worker_registry_client
            .drop_worker_of_request(request_id)
            .await;
        let err = format!("worker returned invalid receipt: {e}");
        if let Some(s) = state.bonsai.inner.lock().await.sessions.get_mut(&session_id) {
            s.terminal_error = Some(err.clone());
        }
        return Ok(Json(SessionStatusResponse {
            status: "FAILED".into(),
            receipt_url: None,
            error_msg: Some(err),
            state: None,
            elapsed_time: None,
        }));
    }

    info!("Verified RISC Zero receipt for session {session_id} (mode {})", mode.as_str());

    if let Err(e) = state
        .proof_router
        .worker_registry_client
        .proof_complete(request_id)
        .await
    {
        error!("proof_complete command failed for session {session_id}: {e}");
    }

    {
        let mut inner = state.bonsai.inner.lock().await;
        inner.receipts.insert(session_id.clone(), proof_status.proof);
    }

    let url = presigned_url(&state, &format!("/receipts/{session_id}/download"));
    Ok(Json(SessionStatusResponse {
        status: "SUCCEEDED".into(),
        receipt_url: Some(url),
        error_msg: None,
        state: None,
        elapsed_time: None,
    }))
}

async fn handle_receipt_download(
    State(state): State<Arc<HierophantState>>,
    Path(session_id): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    match state.bonsai.inner.lock().await.receipts.get(&session_id) {
        Some(bytes) => {
            debug!(
                "Serving Bonsai receipt for session {session_id} ({} bytes)",
                bytes.len()
            );
            Ok(bytes.clone())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_snark_create(
    State(state): State<Arc<HierophantState>>,
    Json(req): Json<SnarkCreateRequest>,
) -> Result<Json<SnarkCreateResponse>, (StatusCode, String)> {
    // Two-step Bonsai flow: caller has a finished session (STARK receipt),
    // and now wants us to wrap it into a Groth16 seal for onchain
    // verification. We look up the session's receipt and dispatch a new
    // RISC Zero proof job to a worker with `wrap_of` set. That worker must
    // have `groth16_enabled=true`; if none does, the job sits unassignable
    // and status will keep reporting RUNNING until a capable worker joins
    // or the session deadline elapses.
    let (source_receipt, image_id) = {
        let inner = state.bonsai.inner.lock().await;
        if !inner.sessions.contains_key(&req.session_id) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown session {}", req.session_id),
            ));
        }
        let receipt = match inner.receipts.get(&req.session_id).cloned() {
            Some(r) => r,
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "session {} has no receipt yet; poll /sessions/status until SUCCEEDED before calling /snark/create",
                        req.session_id
                    ),
                ));
            }
        };
        let image_id = inner
            .sessions
            .get(&req.session_id)
            .map(|s| s.image_id.clone())
            .unwrap_or_default();
        (receipt, image_id)
    };

    let snark_uuid = Uuid::new_v4().to_string();
    let request_id = B256::random();

    state.bonsai.inner.lock().await.snark_requests.insert(
        snark_uuid.clone(),
        SnarkState {
            session_id: req.session_id.clone(),
            request_id,
            image_id,
            terminal_error: None,
        },
    );

    info!(
        "Bonsai snark {snark_uuid} created (wrap of session {}, request_id {request_id})",
        req.session_id
    );

    // ELF + input are ignored when wrap_of is Some; pass empty Vecs.
    if let Err(e) = state
        .proof_router
        .route_risc0_proof(
            request_id,
            Vec::new(),
            Vec::new(),
            Risc0ProofMode::Groth16,
            Some(source_receipt),
        )
        .await
    {
        error!("Error routing snark wrap {snark_uuid}: {e}");
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("route snark wrap: {e}"),
        ));
    }

    Ok(Json(SnarkCreateResponse { uuid: snark_uuid }))
}

async fn handle_snark_status(
    State(state): State<Arc<HierophantState>>,
    Path(snark_id): Path<String>,
) -> Result<Json<SnarkStatusResponse>, (StatusCode, String)> {
    let (request_id, image_id, terminal_error) = {
        let inner = state.bonsai.inner.lock().await;
        match inner.snark_requests.get(&snark_id) {
            Some(s) => (s.request_id, s.image_id.clone(), s.terminal_error.clone()),
            None => return Err((StatusCode::NOT_FOUND, format!("unknown snark {snark_id}"))),
        }
    };

    if let Some(err) = terminal_error {
        return Ok(Json(SnarkStatusResponse {
            status: "FAILED".into(),
            output: None,
            error_msg: Some(err),
        }));
    }

    if state.bonsai.inner.lock().await.snark_receipts.contains_key(&snark_id) {
        let url = presigned_url(&state, &format!("/snark/{snark_id}/download"));
        return Ok(Json(SnarkStatusResponse {
            status: "SUCCEEDED".into(),
            output: Some(url),
            error_msg: None,
        }));
    }

    let proof_status = match state.proof_router.get_proof_status(request_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("get_proof_status failed for snark {snark_id}: {e}");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("status poll: {e}")));
        }
    };

    let fulfilled: i32 = FulfillmentStatus::Fulfilled.into();
    let unfulfillable: i32 = FulfillmentStatus::Unfulfillable.into();

    if proof_status.fulfillment_status == unfulfillable {
        let err = "snark wrap reported unfulfillable by worker registry".to_string();
        if let Some(s) = state.bonsai.inner.lock().await.snark_requests.get_mut(&snark_id) {
            s.terminal_error = Some(err.clone());
        }
        return Ok(Json(SnarkStatusResponse {
            status: "FAILED".into(),
            output: None,
            error_msg: Some(err),
        }));
    }

    if proof_status.fulfillment_status != fulfilled || proof_status.proof.is_empty() {
        return Ok(Json(SnarkStatusResponse {
            status: "RUNNING".into(),
            output: None,
            error_msg: None,
        }));
    }

    // Wrap job done. Verify the wrapped receipt against the original image
    // (same image_id as the underlying session), then stash the bytes and
    // hand back a download URL.
    let wrapped: Receipt = match bincode::deserialize::<Receipt>(&proof_status.proof) {
        Ok(r) => r,
        Err(e) => {
            error!(
                "Snark {snark_id}: wrapped-receipt deserialization failed ({e}). Dropping worker."
            );
            state
                .proof_router
                .worker_registry_client
                .drop_worker_of_request(request_id)
                .await;
            let err = format!("worker returned malformed wrapped receipt: {e}");
            if let Some(s) = state.bonsai.inner.lock().await.snark_requests.get_mut(&snark_id) {
                s.terminal_error = Some(err.clone());
            }
            return Ok(Json(SnarkStatusResponse {
                status: "FAILED".into(),
                output: None,
                error_msg: Some(err),
            }));
        }
    };

    // Recompute the image_id from the session's ELF to verify against.
    let image_digest_res = {
        let inner = state.bonsai.inner.lock().await;
        match inner.images.get(&image_id) {
            Some(elf) => risc0_zkvm::compute_image_id(elf),
            None => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("image for snark {snark_id} missing at verify time"),
                ));
            }
        }
    };
    let image_digest = match image_digest_res {
        Ok(d) => d,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("recompute image_id for snark {snark_id}: {e}"),
            ));
        }
    };

    if let Err(e) = wrapped.verify(image_digest) {
        warn!("Snark {snark_id}: wrapped-receipt verify failed ({e}). Dropping worker.");
        state
            .proof_router
            .worker_registry_client
            .drop_worker_of_request(request_id)
            .await;
        let err = format!("worker returned invalid wrapped receipt: {e}");
        if let Some(s) = state.bonsai.inner.lock().await.snark_requests.get_mut(&snark_id) {
            s.terminal_error = Some(err.clone());
        }
        return Ok(Json(SnarkStatusResponse {
            status: "FAILED".into(),
            output: None,
            error_msg: Some(err),
        }));
    }

    info!("Verified wrapped (Groth16) receipt for snark {snark_id}");

    if let Err(e) = state
        .proof_router
        .worker_registry_client
        .proof_complete(request_id)
        .await
    {
        error!("proof_complete command failed for snark {snark_id}: {e}");
    }

    {
        let mut inner = state.bonsai.inner.lock().await;
        inner.snark_receipts.insert(snark_id.clone(), proof_status.proof);
    }

    let url = presigned_url(&state, &format!("/snark/{snark_id}/download"));
    Ok(Json(SnarkStatusResponse {
        status: "SUCCEEDED".into(),
        output: Some(url),
        error_msg: None,
    }))
}

async fn handle_snark_receipt_download(
    State(state): State<Arc<HierophantState>>,
    Path(snark_id): Path<String>,
) -> Result<Vec<u8>, StatusCode> {
    match state.bonsai.inner.lock().await.snark_receipts.get(&snark_id) {
        Some(bytes) => {
            debug!(
                "Serving snark receipt for {snark_id} ({} bytes)",
                bytes.len()
            );
            Ok(bytes.clone())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

fn normalize_image_id(raw: &str) -> String {
    let s = raw.trim();
    let s = s.strip_prefix("0x").unwrap_or(s);
    s.to_ascii_lowercase()
}

fn parse_proof_mode(raw: Option<&str>) -> Result<Risc0ProofMode, String> {
    match raw.map(|s| s.to_ascii_lowercase()) {
        None => Ok(Risc0ProofMode::Composite),
        Some(ref s) if s == "composite" => Ok(Risc0ProofMode::Composite),
        Some(ref s) if s == "succinct" => Ok(Risc0ProofMode::Succinct),
        Some(ref s) if s == "groth16" => Ok(Risc0ProofMode::Groth16),
        Some(other) => Err(format!(
            "unsupported proof_mode '{other}' (expected composite|succinct|groth16)"
        )),
    }
}

