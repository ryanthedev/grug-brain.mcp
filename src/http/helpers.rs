//! Shared HTTP helpers: `ApiError`, `call_db`, and `csrf_probe`.
//!
//! All axum handler files in `src/http/` import from this module for the
//! error type and the DB channel bridge.

use crate::server::DbRequest;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(json!({"error": self.message}))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Channel plumbing — sends a `__http/*` request and parses the JSON reply.
// ---------------------------------------------------------------------------

pub async fn call_db(
    db_tx: &mpsc::Sender<DbRequest>,
    tool: &str,
    params: Value,
) -> Result<Value, ApiError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    db_tx
        .send(DbRequest {
            tool: tool.to_string(),
            params,
            reply: reply_tx,
        })
        .await
        .map_err(|_| ApiError::internal("db channel closed"))?;
    let reply = reply_rx
        .await
        .map_err(|_| ApiError::internal("db worker dropped"))?
        .map_err(ApiError::internal)?;
    serde_json::from_str(&reply).map_err(|e| ApiError::internal(format!("parse json: {e}")))
}

// ---------------------------------------------------------------------------
// CSRF probe (kept for backward compat with existing CSRF tests).
// ---------------------------------------------------------------------------

pub async fn csrf_probe() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"ok": true})))
}
