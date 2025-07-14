use crate::worker_state::WorkerState;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use std::sync::Arc;

pub fn create_router(state: Arc<WorkerState>) -> Router {
    Router::new()
        // Health check.  Returns true indicating that http has started
        .route("/health", get(handle_health))
        .with_state(state)
}

// Handler for simple health check
async fn handle_health(State(state): State<Arc<WorkerState>>) -> Result<Json<bool>, StatusCode> {
    // ready is set to true in `api/connect_to_hierophant`
    let ready = *state.ready.lock().await;

    Ok(Json(ready))
}
