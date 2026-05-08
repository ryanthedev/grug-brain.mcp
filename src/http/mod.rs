//! In-process HTTP server bound to 127.0.0.1.
//!
//! Co-runs with the MCP socket inside `run_server`. All DB access flows
//! through the same `DbRequest` channel as the socket layer (see
//! `server::dispatch_tool`'s `__http/*` arms), preserving the single-writer
//! invariant.
//!
//! Defenses:
//! - Host header allowlist (only `localhost` / `127.0.0.1`)
//! - CORS lockdown to same-origin
//! - `X-Grug-Client: web` required on mutating routes (CSRF defense)
//! - Strict CSP on HTML responses

pub mod assets;
pub mod graph;
pub mod helpers;
pub mod memories;
pub mod search;
pub mod security;
pub mod sse;
pub mod write;

use crate::config::expand_home;
use crate::server::DbRequest;
use crate::types::MemoryEvent;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{any, get, post, put};
use axum::Router;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tower_http::trace::TraceLayer;

/// Default HTTP port (overridable via `GRUG_PORT`).
pub const DEFAULT_PORT: u16 = 7777;

/// Path of the chosen-port advertisement file. Honors `GRUG_PORT_FILE` so
/// tests can isolate this side effect without racing on `~/.grug-brain/`.
pub fn default_port_file() -> PathBuf {
    if let Ok(p) = std::env::var("GRUG_PORT_FILE")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    expand_home("~/.grug-brain/serve.port")
}

/// Shared state passed to handlers via axum's `State` extractor.
#[derive(Clone)]
pub struct AppState {
    pub db_tx: mpsc::Sender<DbRequest>,
    /// Sender for `MemoryEvent`s. `None` if the watcher failed to start;
    /// SSE handler degrades gracefully (replies with an empty stream).
    pub events: Option<broadcast::Sender<MemoryEvent>>,
}

/// Build the axum router. Public so integration tests can mount it without a
/// real listener.
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/brains", get(memories::brains))
        .route("/api/memories", get(memories::memories))
        .route("/api/memory/:brain/:category/:path", get(memories::memory))
        .route("/api/graph", get(graph::graph))
        .route("/api/search", get(search::search))
        .route("/api/quickswitch", get(search::quickswitch))
        .route("/api/healthz", get(memories::healthz))
        .route("/api/events", get(sse::events))
        // Phase 6: tags / backlinks / local-graph.
        .route("/api/tags", get(memories::tags))
        .route("/api/backlinks", get(memories::backlinks))
        .route("/api/graph/local", get(graph::graph_local))
        // Write routes (Plan 2 Phase 1).
        .route(
            "/api/memory/:brain/:category/:path",
            put(write::memory_write).delete(write::memory_delete),
        )
        .route(
            "/api/memory/:brain/:category/:path/rename",
            post(write::memory_rename),
        )
        .route("/api/memory", post(write::memory_create))
        // CSRF probe (kept for backward compat).
        .route("/api/_csrf_probe", any(helpers::csrf_probe));

    Router::new()
        .merge(api)
        .fallback(any(assets::serve_asset))
        // Layers apply bottom-up: the LAST one added is OUTERMOST. Order chosen
        // so that host_allowlist runs first (cheap, rejects early), then CSRF,
        // then CORS, then CSP is added on the way out.
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(security::add_csp_header))
        .layer(security::cors_layer())
        .layer(middleware::from_fn(security::require_client_header))
        .layer(middleware::from_fn(security::host_allowlist))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(Arc::new(state))
}

/// Bind a TCP listener with port-collision fallback.
///
/// Tries the requested port first. On `AddrInUse`, falls back to an ephemeral
/// port (kernel-assigned). Returns the listener and the actual bound port.
pub async fn bind_listener(preferred_port: u16) -> Result<(TcpListener, u16), String> {
    let primary = SocketAddr::from(([127, 0, 0, 1], preferred_port));
    match TcpListener::bind(primary).await {
        Ok(l) => {
            let port = l.local_addr().map(|a| a.port()).unwrap_or(preferred_port);
            Ok((l, port))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && preferred_port != 0 => {
            let fallback = SocketAddr::from(([127, 0, 0, 1], 0));
            let listener = TcpListener::bind(fallback)
                .await
                .map_err(|e| format!("bind ephemeral: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("local_addr: {e}"))?
                .port();
            Ok((listener, port))
        }
        Err(e) => Err(format!("bind 127.0.0.1:{preferred_port}: {e}")),
    }
}

/// Write the chosen port to `~/.grug-brain/serve.port` (decimal, no newline
/// requirement). Best-effort: errors are logged and ignored — the server
/// works without the advertisement file.
pub fn write_port_file(path: &std::path::Path, port: u16) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, port.to_string()) {
        eprintln!("grug: failed to write port file {}: {e}", path.display());
    }
}

/// Remove the port file. No-op if it doesn't exist.
pub fn remove_port_file(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Run the HTTP server until `shutdown_rx` resolves. Consumes the listener.
pub async fn run_http(
    listener: TcpListener,
    state: AppState,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), String> {
    let app = build_router(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| format!("http serve: {e}"))
}

/// Read the configured HTTP port, honoring `GRUG_PORT`.
pub fn configured_port() -> u16 {
    std::env::var("GRUG_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    /// DW-3.1: `handlers.rs` must be deleted; axum handlers live in the split
    /// files. We verify by checking that the split files exist (compiled into
    /// the binary via include_str!) and that handlers.rs is NOT declared.
    #[test]
    fn test_DW_3_1_handlers_rs_deleted_or_tiny() {
        const MOD_SRC: &str = include_str!("mod.rs");
        // handlers must not be a declared submodule anymore.
        // Note: check the `pub mod` lines only (not this test's string literals).
        let has_handlers_mod = MOD_SRC
            .lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with('"'))
            .any(|l| l.trim() == "pub mod handlers;");
        assert!(
            !has_handlers_mod,
            "DW-3.1: src/http/mod.rs must not declare a handlers submodule — \
             handlers.rs should be deleted"
        );
        // Split files must be declared.
        for module in &["memories", "search", "graph", "write", "helpers"] {
            let needle = format!("pub mod {module};");
            assert!(
                MOD_SRC.contains(&needle),
                "DW-3.1: src/http/mod.rs must declare `{needle}`"
            );
        }
    }

    #[test]
    fn test_DW_3_1_axum_handlers_split_into_files() {
        // Confirm the split files compile and contain the expected handlers
        // by reading them at compile time and grepping for key function names.
        const MEMORIES_SRC: &str = include_str!("memories.rs");
        const SEARCH_SRC: &str = include_str!("search.rs");
        const GRAPH_SRC: &str = include_str!("graph.rs");
        const WRITE_SRC: &str = include_str!("write.rs");

        let checks: &[(&str, &str, &str)] = &[
            ("memories.rs", MEMORIES_SRC, "pub async fn brains"),
            ("memories.rs", MEMORIES_SRC, "pub async fn memories"),
            ("memories.rs", MEMORIES_SRC, "pub async fn memory"),
            ("memories.rs", MEMORIES_SRC, "pub async fn healthz"),
            ("search.rs", SEARCH_SRC, "pub async fn search"),
            ("search.rs", SEARCH_SRC, "pub async fn quickswitch"),
            ("graph.rs", GRAPH_SRC, "pub async fn graph"),
            ("graph.rs", GRAPH_SRC, "pub async fn graph_local"),
            ("write.rs", WRITE_SRC, "pub async fn memory_write"),
            ("write.rs", WRITE_SRC, "pub async fn memory_create"),
            ("write.rs", WRITE_SRC, "pub async fn memory_delete"),
            ("write.rs", WRITE_SRC, "pub async fn memory_rename"),
        ];
        let mut failures = Vec::new();
        for (file, src, needle) in checks {
            if !src.contains(needle) {
                failures.push(format!("{file}: missing `{needle}`"));
            }
        }
        assert!(
            failures.is_empty(),
            "DW-3.1: expected handler functions not found:\n{}",
            failures.join("\n")
        );
    }

    /// DW-3.4: No new file in `src/http/` exceeds 300 lines.
    #[test]
    fn test_DW_3_4_no_http_file_exceeds_300_lines() {
        let files: &[(&str, &str)] = &[
            ("helpers.rs", include_str!("helpers.rs")),
            ("memories.rs", include_str!("memories.rs")),
            ("search.rs", include_str!("search.rs")),
            ("graph.rs", include_str!("graph.rs")),
            ("write.rs", include_str!("write.rs")),
            ("mod.rs", include_str!("mod.rs")),
        ];
        let mut over = Vec::new();
        for (name, src) in files {
            let lines = src.lines().count();
            if lines > 300 {
                over.push(format!("{name}: {lines} lines"));
            }
        }
        assert!(
            over.is_empty(),
            "DW-3.4: http files must each be ≤300 lines — over-limit:\n{}",
            over.join("\n")
        );
    }
}
