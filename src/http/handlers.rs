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
use axum::http::StatusCode;
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

pub async fn healthz(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let v = call_db(&state.db_tx, "__http/healthz", json!({})).await?;
    Ok(Json(v))
}

/// Mutating-route placeholder. Plan 2 will replace this with real write
/// endpoints. The CSRF middleware will already have rejected the request if
/// the `X-Grug-Client: web` header is missing, so reaching this body means
/// the header was present.
pub async fn csrf_probe() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"ok": true})))
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
