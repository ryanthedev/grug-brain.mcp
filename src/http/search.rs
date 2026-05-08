//! Axum handlers for full-text search and quickswitch endpoints.

use super::helpers::{call_db, ApiError};
use super::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub brain: Option<String>,
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Query(p): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = call_db(
        &state.db_tx,
        "__http/search",
        json!({"q": p.q.unwrap_or_default(), "brain": p.brain}),
    )
    .await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct QuickswitchQuery {
    pub q: Option<String>,
}

pub async fn quickswitch(
    State(state): State<Arc<AppState>>,
    Query(p): Query<QuickswitchQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = call_db(
        &state.db_tx,
        "__http/quickswitch",
        json!({"q": p.q.unwrap_or_default()}),
    )
    .await?;
    Ok(Json(v))
}
