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
pub mod handlers;
pub mod security;
pub mod sse;

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
    if let Ok(p) = std::env::var("GRUG_PORT_FILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
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
        .route("/api/brains", get(handlers::brains))
        .route("/api/memories", get(handlers::memories))
        .route("/api/memory/:brain/:category/:path", get(handlers::memory))
        .route("/api/graph", get(handlers::graph))
        .route("/api/search", get(handlers::search))
        .route("/api/quickswitch", get(handlers::quickswitch))
        .route("/api/healthz", get(handlers::healthz))
        .route("/api/events", get(sse::events))
        // Write routes (Plan 2 Phase 1).
        .route(
            "/api/memory/:brain/:category/:path",
            put(handlers::memory_write).delete(handlers::memory_delete),
        )
        .route(
            "/api/memory/:brain/:category/:path/rename",
            post(handlers::memory_rename),
        )
        .route("/api/memory", post(handlers::memory_create))
        // CSRF probe (kept for backward compat).
        .route("/api/_csrf_probe", any(handlers::csrf_probe));

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
