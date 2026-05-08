//! Axum handlers for memory-listing, brain-listing, tags, backlinks, and
//! health check endpoints.

use super::helpers::{call_db, ApiError};
use super::AppState;
use crate::helpers::validate_memory_path;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn brains(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let v = call_db(&state.db_tx, "__http/brains", json!({})).await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct MemoriesQuery {
    pub brain: Option<String>,
}

pub async fn memories(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MemoriesQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = call_db(
        &state.db_tx,
        "__http/memories",
        json!({"brain": q.brain}),
    )
    .await?;
    Ok(Json(v))
}

pub async fn memory(
    State(state): State<Arc<AppState>>,
    axum::extract::Path((brain, category, path)): axum::extract::Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    // Path traversal defense. Reject before sending to worker.
    validate_memory_path(&category).map_err(ApiError::bad_request)?;
    validate_memory_path(&path).map_err(ApiError::bad_request)?;

    let v = call_db(
        &state.db_tx,
        "__http/memory",
        json!({"brain": brain, "category": category, "path": path}),
    )
    .await?;
    if v.get("not_found").and_then(|x| x.as_bool()).unwrap_or(false) {
        return Err(ApiError::not_found("memory not found"));
    }
    Ok(Json(v))
}

/// Query parameters for `/api/healthz`.
///
/// In debug builds, `__test_force_500=1` triggers a synthetic 500 response
/// so Playwright tests can exercise the error toast without a real failure.
/// The field does not exist in release builds so it is never accessible in
/// production — a `#[cfg(debug_assertions)]` field is silently absent in
/// the compiled struct, meaning release binaries simply ignore any such param.
#[derive(Debug, Deserialize)]
pub struct HealthzQuery {
    #[cfg(debug_assertions)]
    pub __test_force_500: Option<String>,
}

pub async fn healthz(
    State(state): State<Arc<AppState>>,
    Query(_q): Query<HealthzQuery>,
) -> Result<Json<Value>, ApiError> {
    // DW-4.11: In debug builds, ?__test_force_500=1 triggers a synthetic 500
    // so Playwright tests can exercise the error toast without a real failure.
    // This param is ignored in release builds.
    #[cfg(debug_assertions)]
    if _q.__test_force_500.as_deref() == Some("1") {
        return Err(ApiError::internal("forced test error"));
    }

    let v = call_db(&state.db_tx, "__http/healthz", json!({})).await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct TagsQuery {
    pub brain: Option<String>,
}

/// GET /api/tags?brain=... → `[{"tag": String, "count": i64}]`.
/// Aggregates the indexer-populated `tags` table.
pub async fn tags(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TagsQuery>,
) -> Result<Json<Value>, ApiError> {
    let v = call_db(&state.db_tx, "__http/tags", json!({"brain": q.brain})).await?;
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
pub struct BacklinksQuery {
    pub brain: Option<String>,
    pub path: String,
}

/// GET /api/backlinks?brain=&path= → array of memories that wikilink to `{brain, path}`.
/// `path` is a brain-relative `category/name.md`. Validated against traversal.
pub async fn backlinks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<BacklinksQuery>,
) -> Result<Json<Value>, ApiError> {
    // Path traversal defense — both segments separately.
    let (cat, file) = q
        .path
        .rsplit_once('/')
        .ok_or_else(|| ApiError::bad_request("path must be category/name.md"))?;
    validate_memory_path(cat).map_err(ApiError::bad_request)?;
    validate_memory_path(file).map_err(ApiError::bad_request)?;
    let v = call_db(
        &state.db_tx,
        "__http/backlinks",
        json!({"brain": q.brain, "path": q.path}),
    )
    .await?;
    Ok(Json(v))
}
