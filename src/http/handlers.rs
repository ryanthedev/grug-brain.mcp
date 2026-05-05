//! Read-only HTTP handlers + the mutating-route placeholder.
//!
//! Each handler is a thin axum wrapper that:
//!   1. validates input (path traversal etc.)
//!   2. sends a `__http/*` request through `db_tx`
//!   3. parses the JSON reply and returns it
//!
//! The DB-thread side of these requests lives in this same module
//! (`brains_json`, `memories_json`, ...) and is wired into `dispatch_tool`
//! in `server.rs`.

use super::AppState;
use crate::db::SCHEMA_VERSION;
use crate::helpers::validate_memory_path;
use crate::server::DbRequest;
use crate::tools::search::search_all;
use crate::tools::similarity::find_similar;
use crate::tools::GrugDb;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::oneshot;

// ---------------------------------------------------------------------------
// axum handlers
// ---------------------------------------------------------------------------

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
    Path((brain, category, path)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    // Path traversal defense (DW-3.10). Reject before sending to worker.
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

/// Mutating-route placeholder (kept for backward compat with existing CSRF tests).
pub async fn csrf_probe() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"ok": true})))
}

// ---------------------------------------------------------------------------
// Write handlers (Plan 2 Phase 1)
// ---------------------------------------------------------------------------

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

    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        if err == "read-only brain" {
            return Ok((StatusCode::FORBIDDEN, Json(v)).into_response());
        }
    }

    // Idempotent: return 204 whether the file existed or not.
    Ok((StatusCode::NO_CONTENT, ()).into_response())
}

/// Request body for POST /api/memory/:brain/:category/:path/rename
#[derive(Debug, Deserialize)]
pub struct MemoryRenameBody {
    pub new_path: String,
}

/// POST /api/memory/:brain/:category/:path/rename
/// Renames the memory (no link rewrite). Returns 200 + {path, etag}.
pub async fn memory_rename(
    State(state): State<Arc<AppState>>,
    Path((brain, category, path)): Path<(String, String, String)>,
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

    let v = call_db(
        &state.db_tx,
        "__http/memory_rename",
        json!({
            "brain": brain,
            "old_rel_path": old_rel,
            "new_rel_path": body.new_path,
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

    Ok((StatusCode::OK, Json(v)).into_response())
}

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

async fn call_db(
    db_tx: &tokio::sync::mpsc::Sender<DbRequest>,
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
// DB-worker side: produce JSON strings for each route. Called from
// `dispatch_tool` arms named `__http/*`.
// ---------------------------------------------------------------------------

pub fn brains_json(db: &mut GrugDb) -> Result<String, String> {
    db.maybe_reload_config();
    let arr: Vec<Value> = db
        .config()
        .brains
        .iter()
        .map(|b| {
            json!({
                "name": b.name,
                "primary": b.primary,
                "writable": b.writable,
                "source": b.source,
                "flat": b.flat,
            })
        })
        .collect();
    serde_json::to_string(&Value::Array(arr)).map_err(|e| e.to_string())
}

pub fn memories_json(db: &mut GrugDb, brain: Option<&str>) -> Result<String, String> {
    db.maybe_reload_config();
    let target = match brain {
        Some(name) => Some(db.resolve_brain(Some(name))?.name.clone()),
        None => None,
    };

    let (sql, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match &target {
        Some(name) => (
            "SELECT path, brain, category, name, description, date FROM brain_fts WHERE brain = ?1 ORDER BY category, date DESC",
            vec![name as &dyn rusqlite::types::ToSql],
        ),
        None => (
            "SELECT path, brain, category, name, description, date FROM brain_fts ORDER BY brain, category, date DESC",
            vec![],
        ),
    };


    let mut stmt = db.conn().prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params.as_slice(), |row| {
            let path: String = row.get(0)?;
            let brain: String = row.get(1)?;
            let category: String = row.get(2)?;
            let name: String = row.get(3)?;
            let description: String = row.get(4)?;
            let date: String = row.get(5)?;
            Ok((path, brain, category, name, description, date))
        })
        .map_err(|e| e.to_string())?;

    let mut out: Vec<Value> = Vec::new();
    for r in rows {
        let (path, brain, category, name, description, date) = r.map_err(|e| e.to_string())?;
        // mtime from files table
        let mtime: f64 = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
                rusqlite::params![&brain, &path],
                |row| row.get(0),
            )
            .unwrap_or(0.0);
        out.push(json!({
            "path": path,
            "brain": brain,
            "category": category,
            "name": name,
            "description": description,
            "date": date,
            "mtime": mtime,
        }));
    }
    serde_json::to_string(&Value::Array(out)).map_err(|e| e.to_string())
}

pub fn memory_json(
    db: &mut GrugDb,
    brain: &str,
    category: &str,
    path: &str,
) -> Result<String, String> {
    db.maybe_reload_config();
    validate_memory_path(category)?;
    validate_memory_path(path)?;

    let brain_obj = db.resolve_brain(Some(brain))?.clone();
    let file_name = if path.ends_with(".md") {
        path.to_string()
    } else {
        format!("{path}.md")
    };
    let rel_path = if brain_obj.flat {
        file_name.clone()
    } else {
        format!("{category}/{file_name}")
    };
    let abs_path = brain_obj.dir.join(&rel_path);
    if !abs_path.exists() {
        return Ok(serde_json::to_string(&json!({"not_found": true})).unwrap());
    }

    let content = std::fs::read_to_string(&abs_path)
        .map_err(|e| format!("read {}: {e}", abs_path.display()))?;
    let frontmatter = crate::parsing::extract_frontmatter(&content);
    let body = crate::parsing::extract_body(&content);
    let mtime: f64 = db
        .conn()
        .query_row(
            "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
            rusqlite::params![&brain_obj.name, &rel_path],
            |row| row.get(0),
        )
        .unwrap_or(0.0);

    let neighbors = find_similar(db.conn(), &brain_obj.name, &rel_path, 10)
        .unwrap_or_default()
        .into_iter()
        .map(|s| json!({"path": s.path, "brain": s.brain, "score": s.score}))
        .collect::<Vec<_>>();

    let v = json!({
        "frontmatter": frontmatter,
        "body": body,
        "mtime": mtime,
        "neighbors": neighbors,
    });
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

pub fn graph_json(
    db: &mut GrugDb,
    brain: Option<&str>,
    _mode: Option<&str>,
    _node: Option<&str>,
    _depth: Option<usize>,
) -> Result<String, String> {
    db.maybe_reload_config();
    // Default cosine threshold and edge cap.
    const SCORE_THRESHOLD: f64 = 0.1;
    const EDGE_CAP: usize = 1000;

    // Nodes: every memory in the (optionally filtered) FTS table.
    let brain_owned: Option<String> = brain.map(|s| s.to_string());
    let (node_sql, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match &brain_owned {
        Some(name) => (
            "SELECT brain, path, category, name FROM brain_fts WHERE brain = ?1",
            vec![name as &dyn rusqlite::types::ToSql],
        ),
        None => (
            "SELECT brain, path, category, name FROM brain_fts",
            vec![],
        ),
    };
    let mut stmt = db.conn().prepare(node_sql).map_err(|e| e.to_string())?;
    let nodes: Vec<Value> = stmt
        .query_map(params.as_slice(), |row| {
            Ok(json!({
                "brain": row.get::<_, String>(0)?,
                "path": row.get::<_, String>(1)?,
                "category": row.get::<_, String>(2)?,
                "name": row.get::<_, String>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    // Similarity edges from cross_links.
    let mut sim_stmt = db
        .conn()
        .prepare(
            "SELECT brain_a, path_a, brain_b, path_b, score FROM cross_links \
             WHERE score >= ?1 ORDER BY score DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let sim_edges: Vec<Value> = sim_stmt
        .query_map(
            rusqlite::params![SCORE_THRESHOLD, EDGE_CAP as i64],
            |row| {
                Ok(json!({
                    "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                    "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                    "kind": "similarity",
                    "score": row.get::<_, f64>(4)?,
                }))
            },
        )
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    // Explicit wikilink edges from links table (resolved targets only).
    let mut link_stmt = db
        .conn()
        .prepare(
            "SELECT brain, src_path, target_brain, target_path FROM links \
             WHERE target_brain IS NOT NULL AND target_path IS NOT NULL LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let link_edges: Vec<Value> = link_stmt
        .query_map(rusqlite::params![EDGE_CAP as i64], |row| {
            Ok(json!({
                "src": {"brain": row.get::<_, String>(0)?, "path": row.get::<_, String>(1)?},
                "dst": {"brain": row.get::<_, String>(2)?, "path": row.get::<_, String>(3)?},
                "kind": "explicit",
                "score": 1.0,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut edges = sim_edges;
    edges.extend(link_edges);
    if edges.len() > EDGE_CAP {
        edges.truncate(EDGE_CAP);
    }

    serde_json::to_string(&json!({"nodes": nodes, "edges": edges}))
        .map_err(|e| e.to_string())
}

pub fn search_json(
    db: &mut GrugDb,
    query: &str,
    brain: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let (results, total) = search_all(db.conn(), query, None);
    let filtered: Vec<&_> = match brain {
        Some(b) => results.iter().filter(|r| r.brain == b).collect(),
        None => results.iter().collect(),
    };
    let hits: Vec<Value> = filtered
        .iter()
        .map(|r| {
            json!({
                "path": r.path,
                "brain": r.brain,
                "category": r.category,
                "name": r.name,
                "date": r.date,
                "description": r.description,
                "snippet": r.snippet,
                "rank": r.rank,
            })
        })
        .collect();
    serde_json::to_string(&json!({"hits": hits, "total": total}))
        .map_err(|e| e.to_string())
}

pub fn quickswitch_json(db: &mut GrugDb, query: &str) -> Result<String, String> {
    db.maybe_reload_config();
    // Name-prefix match across all brains. SQL LIKE; case-insensitive via
    // FTS's lowercase index column.
    let pattern = format!("%{}%", query.to_lowercase());
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT path, brain, category, name FROM brain_fts \
             WHERE LOWER(name) LIKE ?1 ORDER BY name LIMIT 50",
        )
        .map_err(|e| e.to_string())?;
    let hits: Vec<Value> = stmt
        .query_map([&pattern], |row| {
            Ok(json!({
                "path": row.get::<_, String>(0)?,
                "brain": row.get::<_, String>(1)?,
                "category": row.get::<_, String>(2)?,
                "name": row.get::<_, String>(3)?,
            }))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    serde_json::to_string(&json!({"hits": hits})).map_err(|e| e.to_string())
}

pub fn healthz_json(db: &mut GrugDb) -> Result<String, String> {
    db.maybe_reload_config();
    let last_index_at: Option<f64> = db
        .conn()
        .query_row("SELECT MAX(mtime) FROM files", [], |row| row.get(0))
        .ok();
    let brains: Vec<Value> = db
        .config()
        .brains
        .iter()
        .map(|b| {
            // last_sync_at: max mtime across files in this brain.
            let last: Option<f64> = db
                .conn()
                .query_row(
                    "SELECT MAX(mtime) FROM files WHERE brain = ?1",
                    [&b.name],
                    |row| row.get(0),
                )
                .ok();
            json!({"name": b.name, "last_sync_at": last})
        })
        .collect();
    let v = json!({
        "ok": true,
        "schema_version": SCHEMA_VERSION,
        "last_index_at": last_index_at,
        "brains": brains,
    });
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// DB-worker side: write-path JSON producers (Plan 2 Phase 1)
// ---------------------------------------------------------------------------

/// Split a relative path `"category/name"` or `"category/name.md"` into
/// `(category, stem)` where stem has no `.md` extension. Returns Err if the
/// format is invalid.
fn split_rel_path(rel_path: &str) -> Result<(String, String), String> {
    // Strip .md extension if present.
    let stripped = rel_path.strip_suffix(".md").unwrap_or(rel_path);
    // Split on the LAST '/' to handle nested category-like paths gracefully.
    if let Some(pos) = stripped.rfind('/') {
        let cat = stripped[..pos].to_string();
        let name = stripped[pos + 1..].to_string();
        if cat.is_empty() || name.is_empty() {
            return Err(format!("invalid path: {rel_path:?}"));
        }
        Ok((cat, name))
    } else {
        Err(format!("path must be 'category/name', got: {rel_path:?}"))
    }
}

/// Read the current mtime for a path from the `files` table.
fn read_mtime(db: &mut GrugDb, brain_name: &str, rel_path: &str) -> f64 {
    db.conn()
        .query_row(
            "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
            rusqlite::params![brain_name, rel_path],
            |row| row.get(0),
        )
        .unwrap_or(0.0)
}

/// `__http/memory_write`: update an existing memory via PUT with ETag.
///
/// Returns `{ok, etag}` on success, or an error object for conflict/readonly/notfound.
pub fn memory_write_json(
    db: &mut GrugDb,
    brain_name: &str,
    rel_path: &str,
    body: &str,
    frontmatter: Option<&str>,
    if_match_etag: f64,
    attempted_body: &str,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(Some(brain_name))?.clone();

    if !brain.writable {
        let v = json!({"error": "read-only brain", "brain": brain.name});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    let (category, stem) = split_rel_path(rel_path)?;
    validate_memory_path(&category)?;
    validate_memory_path(&stem)?;

    // Build the full file content.
    let file_content = if let Some(fm) = frontmatter {
        if fm.trim().is_empty() {
            body.to_string()
        } else {
            format!("---\n{}\n---\n\n{}", fm.trim_end(), body)
        }
    } else {
        body.to_string()
    };

    // Assemble canonical path for existence check.
    let canonical_rel = format!("{category}/{stem}.md");
    let file_path = brain.dir.join(&category).join(format!("{stem}.md"));

    if !file_path.exists() {
        let v = json!({"error": "not found", "path": canonical_rel});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    // grug_write handles ETag conflict check + atomic write + indexing.
    let result = crate::tools::write::grug_write(
        db,
        &category,
        &stem,
        &file_content,
        Some(brain_name),
        Some(if_match_etag),
    );

    match result {
        Err(conflict_json) => {
            // Parse grug_write's internal conflict shape and reshape for HTTP.
            let inner: Value =
                serde_json::from_str(&conflict_json).unwrap_or(json!({"error": "conflict"}));
            let current_etag = inner.get("current_mtime").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let current_body = inner
                .get("current_content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let v = json!({
                "error": "conflict",
                "current_etag": current_etag,
                "current_body": current_body,
                "attempted_body": attempted_body,
            });
            Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
        }
        Ok(_) => {
            let new_mtime = read_mtime(db, &brain.name, &canonical_rel);
            let v = json!({"ok": true, "etag": new_mtime});
            Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
        }
    }
}

/// `__http/memory_create`: create a new memory via POST.
///
/// Returns `{path, etag}` on 201, or error object for duplicate/readonly.
pub fn memory_create_json(
    db: &mut GrugDb,
    brain_name: Option<&str>,
    rel_path: &str,
    body: &str,
    frontmatter: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();

    if !brain.writable {
        let v = json!({"error": "read-only brain", "brain": brain.name});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    let (category, stem) = split_rel_path(rel_path)?;
    validate_memory_path(&category)?;
    validate_memory_path(&stem)?;

    // Check for duplicate before calling write.
    let canonical_rel = format!("{category}/{stem}.md");
    let file_path = brain.dir.join(&category).join(format!("{stem}.md"));
    if file_path.exists() {
        let v = json!({"error": "duplicate path", "path": canonical_rel});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    let file_content = if let Some(fm) = frontmatter {
        if fm.trim().is_empty() {
            body.to_string()
        } else {
            format!("---\n{}\n---\n\n{}", fm.trim_end(), body)
        }
    } else {
        body.to_string()
    };

    // No ETag for create (new file).
    crate::tools::write::grug_write(
        db,
        &category,
        &stem,
        &file_content,
        Some(&brain.name),
        None,
    )?;

    let new_mtime = read_mtime(db, &brain.name, &canonical_rel);
    let v = json!({"path": canonical_rel, "etag": new_mtime});
    Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
}

/// `__http/memory_delete`: delete a memory via DELETE.
///
/// Returns `{ok: true}` always (idempotent: missing file is not an error).
pub fn memory_delete_json(
    db: &mut GrugDb,
    brain_name: &str,
    rel_path: &str,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(Some(brain_name))?.clone();

    if !brain.writable {
        let v = json!({"error": "read-only brain", "brain": brain.name});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    let (category, stem) = split_rel_path(rel_path)?;
    validate_memory_path(&category)?;
    validate_memory_path(&stem)?;

    // grug_delete returns Ok("not found: ...") for missing files — that's fine
    // for DELETE idempotency. Hard=false (soft delete to .trash/).
    crate::tools::delete::grug_delete(db, &category, &stem, Some(brain_name), false)
        .map(|_| serde_json::to_string(&json!({"ok": true})).unwrap())
}

/// `__http/memory_rename`: rename a memory via POST .../rename.
///
/// Returns `{path, etag}` on success, or error object for readonly/notfound/duplicate.
pub fn memory_rename_json(
    db: &mut GrugDb,
    brain_name: &str,
    old_rel_path: &str,
    new_rel_path: &str,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(Some(brain_name))?.clone();

    if !brain.writable {
        let v = json!({"error": "read-only brain", "brain": brain.name});
        return Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?);
    }

    let (old_cat, old_stem) = split_rel_path(old_rel_path)?;
    let (new_cat, new_stem) = split_rel_path(new_rel_path)?;
    validate_memory_path(&old_cat)?;
    validate_memory_path(&old_stem)?;
    validate_memory_path(&new_cat)?;
    validate_memory_path(&new_stem)?;

    let result = crate::tools::write::grug_rename(
        db,
        &old_cat,
        &old_stem,
        &new_cat,
        &new_stem,
        Some(brain_name),
    );

    match result {
        Err(e) => {
            // grug_rename returns Err for "source not found" and "destination already exists".
            let (err_kind, msg) = if e.contains("source not found") {
                ("not found", e.clone())
            } else if e.contains("destination already exists") {
                ("destination exists", e.clone())
            } else {
                ("error", e.clone())
            };
            let v = json!({"error": err_kind, "message": msg});
            Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
        }
        Ok(_) => {
            let new_canonical = format!("{new_cat}/{new_stem}.md");
            let new_mtime = read_mtime(db, &brain.name, &new_canonical);
            let v = json!({"path": new_canonical, "etag": new_mtime});
            Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
        }
    }
}
