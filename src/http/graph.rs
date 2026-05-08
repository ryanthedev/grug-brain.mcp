//! Axum handlers for graph and local-graph endpoints.

use super::helpers::{call_db, ApiError};
use super::AppState;
use crate::helpers::validate_memory_path;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct GraphQuery {
    pub brain: Option<String>,
    pub mode: Option<String>,
    pub node: Option<String>,
    pub depth: Option<u64>,
}

pub async fn graph(
    State(state): State<Arc<AppState>>,
    Query(q): Query<GraphQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = call_db(
        &state.db_tx,
        "__http/graph",
        json!({
            "brain": q.brain,
            "mode": q.mode,
            "node": q.node,
            "depth": q.depth,
        }),
    )
    .await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct GraphLocalQuery {
    pub brain: Option<String>,
    pub path: String,
    pub hops: Option<u64>,
}

/// GET /api/graph/local?brain=&path=&hops=N → `{nodes, edges}` for the
/// N-hop neighborhood around `{brain, path}` in the wikilink graph.
/// `hops` defaults to 2 and is capped at 3.
pub async fn graph_local(
    State(state): State<Arc<AppState>>,
    Query(q): Query<GraphLocalQuery>,
) -> Result<Json<Value>, ApiError> {
    let (cat, file) = q
        .path
        .rsplit_once('/')
        .ok_or_else(|| ApiError::bad_request("path must be category/name.md"))?;
    validate_memory_path(cat).map_err(ApiError::bad_request)?;
    validate_memory_path(file).map_err(ApiError::bad_request)?;
    let hops = q.hops.unwrap_or(2).min(3);
    let v = call_db(
        &state.db_tx,
        "__http/graph_local",
        json!({"brain": q.brain, "path": q.path, "hops": hops}),
    )
    .await?;
    Ok(Json(v))
}
