//! Axum handlers for memory write, create, delete, and rename endpoints.

use super::helpers::{call_db, ApiError};
use super::AppState;
use crate::helpers::validate_memory_path;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

/// Request body for PUT /api/memory/:brain/:category/:path
#[derive(Debug, Deserialize)]
pub struct MemoryWriteBody {
    pub body: String,
    pub frontmatter: Option<String>,
}

/// PUT /api/memory/:brain/:category/:path
/// Updates an existing memory. Requires `If-Match: <etag>` header (mtime f64).
/// Returns 200 + {ok, etag} on success, 409 + ConflictResponse on ETag mismatch,
/// 403 on read-only brain.
pub async fn memory_write(
    State(state): State<Arc<AppState>>,
    Path((brain, category, path)): Path<(String, String, String)>,
    headers: HeaderMap,
    Json(body): Json<MemoryWriteBody>,
) -> Result<impl IntoResponse, ApiError> {
    validate_memory_path(&category).map_err(ApiError::bad_request)?;
    validate_memory_path(&path).map_err(ApiError::bad_request)?;

    let if_match: f64 = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| ApiError::bad_request("If-Match header required (ETag as f64)"))?;

    let rel_path = format!("{category}/{path}");

    let v = call_db(
        &state.db_tx,
        "__http/memory_write",
        json!({
            "brain": brain,
            "rel_path": rel_path,
            "body": body.body,
            "frontmatter": body.frontmatter,
            "if_match_etag": if_match,
            "attempted_body": body.body,
        }),
    )
    .await?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        if err == "conflict" {
            return Ok((StatusCode::CONFLICT, Json(v)).into_response());
        }
        if err == "read-only brain" {
            return Ok((StatusCode::FORBIDDEN, Json(v)).into_response());
        }
        if err == "not found" {
            return Ok((StatusCode::NOT_FOUND, Json(v)).into_response());
        }
    }

    Ok((StatusCode::OK, Json(v)).into_response())
}

/// Request body for POST /api/memory (create)
#[derive(Debug, Deserialize)]
pub struct MemoryCreateBody {
    pub path: String,
    pub body: String,
    pub frontmatter: Option<String>,
    /// Optional brain name. Defaults to the primary brain.
    pub brain: Option<String>,
}

/// POST /api/memory
/// Creates a new memory. Returns 201 + {path, etag} on success, 409 if
/// the path already exists, 403 on read-only brain.
pub async fn memory_create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<MemoryCreateBody>,
) -> Result<impl IntoResponse, ApiError> {
    // Validate the path field from the request body.
    // Split on '/' to validate both the category and name parts.
    let parts: Vec<&str> = body.path.splitn(2, '/').collect();
    let (cat, name) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        return Err(ApiError::bad_request("path must be in format 'category/name'"));
    };
    validate_memory_path(cat).map_err(ApiError::bad_request)?;
    validate_memory_path(name).map_err(ApiError::bad_request)?;

    let v = call_db(
        &state.db_tx,
        "__http/memory_create",
        json!({
            "brain": body.brain,
            "rel_path": body.path,
            "body": body.body,
            "frontmatter": body.frontmatter,
        }),
    )
    .await?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        if err == "duplicate path" {
            return Ok((StatusCode::CONFLICT, Json(v)).into_response());
        }
        if err == "read-only brain" {
            return Ok((StatusCode::FORBIDDEN, Json(v)).into_response());
        }
    }

    Ok((StatusCode::CREATED, Json(v)).into_response())
}

/// DELETE /api/memory/:brain/:category/:path
/// Deletes the memory. Returns 204 on success and 204 on missing (idempotent).
/// Returns 403 on read-only brain.
pub async fn memory_delete(
    State(state): State<Arc<AppState>>,
    Path((brain, category, path)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    validate_memory_path(&category).map_err(ApiError::bad_request)?;
    validate_memory_path(&path).map_err(ApiError::bad_request)?;

    let rel_path = format!("{category}/{path}");

    let v = call_db(
        &state.db_tx,
        "__http/memory_delete",
        json!({"brain": brain, "rel_path": rel_path}),
    )
    .await?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str())
        && err == "read-only brain"
    {
        return Ok((StatusCode::FORBIDDEN, Json(v)).into_response());
    }

    // Idempotent: return 204 whether the file existed or not.
    Ok((StatusCode::NO_CONTENT, ()).into_response())
}

/// Request body for POST /api/memory/:brain/:category/:path/rename
#[derive(Debug, Deserialize)]
pub struct MemoryRenameBody {
    pub new_path: String,
}

/// Query parameters for POST /api/memory/:brain/:category/:path/rename.
/// `rewrite_links` defaults to `true`: any `[[old-name]]` references across
/// the brain are rewritten in-place as part of the rename. Pass `false` for
/// the bare-rename escape hatch.
#[derive(Debug, Deserialize, Default)]
pub struct MemoryRenameQuery {
    #[serde(default)]
    pub rewrite_links: Option<bool>,
}

/// POST /api/memory/:brain/:category/:path/rename
/// Renames the memory (and rewrites incoming wikilinks unless
/// `?rewrite_links=false`). Returns 200 + {path, etag, affected_paths}.
pub async fn memory_rename(
    State(state): State<Arc<AppState>>,
    Path((brain, category, path)): Path<(String, String, String)>,
    Query(q): Query<MemoryRenameQuery>,
    Json(body): Json<MemoryRenameBody>,
) -> Result<impl IntoResponse, ApiError> {
    validate_memory_path(&category).map_err(ApiError::bad_request)?;
    validate_memory_path(&path).map_err(ApiError::bad_request)?;

    // Validate new_path parts.
    let parts: Vec<&str> = body.new_path.splitn(2, '/').collect();
    let (new_cat, new_name) = if parts.len() == 2 {
        (parts[0], parts[1])
    } else {
        return Err(ApiError::bad_request("new_path must be in format 'category/name'"));
    };
    validate_memory_path(new_cat).map_err(ApiError::bad_request)?;
    validate_memory_path(new_name).map_err(ApiError::bad_request)?;

    let old_rel = format!("{category}/{path}");

    let rewrite_links = q.rewrite_links.unwrap_or(true);

    let v = call_db(
        &state.db_tx,
        "__http/memory_rename",
        json!({
            "brain": brain,
            "old_rel_path": old_rel,
            "new_rel_path": body.new_path,
            "rewrite_links": rewrite_links,
        }),
    )
    .await?;

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        if err == "read-only brain" {
            return Ok((StatusCode::FORBIDDEN, Json(v)).into_response());
        }
        if err == "not found" {
            return Ok((StatusCode::NOT_FOUND, Json(v)).into_response());
        }
        if err == "destination exists" {
            return Ok((StatusCode::CONFLICT, Json(v)).into_response());
        }
    }

    // Emit a single SSE `reload` event covering every affected path so
    // subscribers don't have to react to per-file watcher events for this
    // multi-file operation.
    if let Some(events) = &state.events {
        let paths: Vec<String> = v
            .get("affected_paths")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        if !paths.is_empty() {
            let _ = events.send(crate::types::MemoryEvent::Reload {
                brain: brain.clone(),
                paths,
                reason: "rename".to_string(),
            });
        }
    }

    Ok((StatusCode::OK, Json(v)).into_response())
}
