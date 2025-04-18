use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use network_lib::ContemplantProofStatus;
use network_lib::ProofRequestId;
use std::collections::HashMap;
use tokio::sync::RwLock;

pub type ProofStore = RwLock<HashMap<ProofRequestId, ContemplantProofStatus>>;

pub struct AppError(pub anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", self.0)).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
