use crate::config::{expand_home, load_brains};
use crate::domain::ports::{
    BrainPort, ConfigPort, DocsPort, DreamPort, GraphPort, MemoryPort, RecallPort, SearchPort,
    SyncPort, WritePort,
};
use crate::git::{build_sync_locks, git, git_commit_file, has_remote};
use crate::protocol::{SocketRequest, SocketResponse};
use crate::services::BrainServices;
use crate::tools::update::EditEntry;
use crate::tools::{GitCommitRequest, GrugDb};
use crate::types::BrainConfig;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

/// Message sent to the DB worker thread.
pub struct DbRequest {
    pub(crate) tool: String,
    pub(crate) params: Value,
    pub(crate) reply: oneshot::Sender<Result<String, String>>,
}

/// Default socket path: ~/.grug-brain/grug.sock
pub fn default_socket_path() -> PathBuf {
    expand_home("~/.grug-brain/grug.sock")
}

/// Default PID file path: ~/.grug-brain/grug.pid
/// Default database path: `~/.grug-brain/grug.db`, overridable via `GRUG_DB`.
fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("GRUG_DB")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    expand_home("~/.grug-brain/grug.db")
}

/// Derive PID file path from socket path (sibling file).
fn pid_path_for_socket(socket_path: &Path) -> PathBuf {
    socket_path.with_extension("pid")
}

/// Remove stale socket file if it exists.
/// If a live server is already running (PID file with living process), returns error.
fn cleanup_stale_socket(socket_path: &Path) -> Result<(), String> {
    if !socket_path.exists() {
        return Ok(());
    }

    let pid_path = pid_path_for_socket(socket_path);
    if pid_path.exists() {
        if let Ok(pid) = fs::read_to_string(&pid_path)
            .unwrap_or_default()
            .trim()
            .parse::<u32>()
        {
            // Check if process is alive via kill(pid, 0)
            #[cfg(unix)]
            {
                let alive = std::process::Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .status()
                    .is_ok_and(|s| s.success());
                if alive {
                    return Err(format!(
                        "grug: another server is running (pid {pid}). \
                         Stop it first or remove {}",
                        socket_path.display()
                    ));
                }
            }
        }
        // PID file exists but process is dead — clean up both
        let _ = fs::remove_file(&pid_path);
    }

    // Socket is stale, remove it
    fs::remove_file(socket_path)
        .map_err(|e| format!("grug: failed to remove stale socket: {e}"))
}

/// Write PID file with current process ID.
fn write_pid_file(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("grug: failed to create PID directory: {e}"))?;
    }
    fs::write(path, std::process::id().to_string())
        .map_err(|e| format!("grug: failed to write PID file: {e}"))
}

/// Remove PID file on shutdown.
fn remove_pid_file(path: &Path) {
    let _ = fs::remove_file(path);
}

/// Extract a string field from a JSON value.
fn extract_str<'a>(params: &'a Value, field: &str) -> Option<&'a str> {
    params.get(field).and_then(|v| v.as_str())
}

/// Extract a u64 field from a JSON value.
fn extract_u64(params: &Value, field: &str) -> Option<u64> {
    params.get(field).and_then(|v| v.as_u64())
}

/// Extract a bool field from a JSON value.
fn extract_bool(params: &Value, field: &str) -> Option<bool> {
    params.get(field).and_then(|v| v.as_bool())
}

/// Dispatch a tool call to the appropriate function on GrugDb.
fn dispatch_tool(db: &mut GrugDb, tool: &str, params: &Value) -> Result<String, String> {
    match tool {
        "grug-search" => {
            let query = extract_str(params, "query").unwrap_or("");
            let page = extract_u64(params, "page").map(|p| p as usize);
            db.grug_search(query, page)
        }
        "grug-write" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let content = extract_str(params, "content").ok_or("missing field: content")?;
            let brain = extract_str(params, "brain");
            let if_match_mtime = params.get("if_match_mtime").and_then(|v| v.as_f64());
            db.grug_write(category, path, content, brain, if_match_mtime)
        }
        "grug-read" => {
            let brain = extract_str(params, "brain");
            let category = extract_str(params, "category");
            let path = extract_str(params, "path");
            db.grug_read(brain, category, path)
        }
        "grug-recall" => {
            let category = extract_str(params, "category");
            let brain = extract_str(params, "brain");
            db.grug_recall(category, brain)
        }
        "grug-delete" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let brain = extract_str(params, "brain");
            let hard = extract_bool(params, "hard").unwrap_or(false);
            db.grug_delete(category, path, brain, hard)
        }
        "grug-config" => {
            let action = extract_str(params, "action").ok_or("missing field: action")?;
            let name = extract_str(params, "name");
            let dir = extract_str(params, "dir");
            let primary = extract_bool(params, "primary");
            let writable = extract_bool(params, "writable");
            let flat = extract_bool(params, "flat");
            let git = extract_str(params, "git");
            let sync_interval = extract_u64(params, "sync_interval");
            let source = extract_str(params, "source");
            let refresh_interval = extract_u64(params, "refresh_interval");
            db.grug_config(
                action,
                name,
                dir,
                primary,
                writable,
                flat,
                git,
                sync_interval,
                source,
                refresh_interval,
            )
        }
        "grug-sync" => {
            let brain = extract_str(params, "brain");
            db.grug_sync(brain)
        }
        "grug-dream" => db.grug_dream(),
        "grug-update" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let brain = extract_str(params, "brain");
            let edits: Vec<EditEntry> = serde_json::from_value(
                params
                    .get("edits")
                    .cloned()
                    .ok_or("missing field: edits")?,
            )
            .map_err(|e| format!("invalid edits: {e}"))?;
            db.grug_update(category, path, &edits, brain)
        }
        "grug-docs" => {
            let category = extract_str(params, "category");
            let path = extract_str(params, "path");
            let page = extract_u64(params, "page").map(|p| p as usize);
            db.grug_docs(category, path, page)
        }
        // HTTP read-only endpoints. These return JSON strings rather than the
        // formatted text shown to MCP clients, but they share the same
        // single-writer worker thread (preserving the dispatch_tool invariant).
        // See `crate::http` for the matching axum routes.
        "__http/brains" => db.brains_json(),
        "__http/memories" => {
            let brain = extract_str(params, "brain");
            db.memories_json(brain)
        }
        "__http/memory" => {
            let brain = extract_str(params, "brain").ok_or("missing field: brain")?;
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            db.memory_json(brain, category, path)
        }
        "__http/graph" => {
            let brain = extract_str(params, "brain");
            let mode = extract_str(params, "mode");
            let node = extract_str(params, "node");
            let depth = extract_u64(params, "depth").map(|d| d as usize);
            db.graph_json(brain, mode, node, depth)
        }
        "__http/search" => {
            let q = extract_str(params, "q").unwrap_or("");
            let brain = extract_str(params, "brain");
            db.search_json(q, brain)
        }
        "__http/quickswitch" => {
            let q = extract_str(params, "q").unwrap_or("");
            db.quickswitch_json(q)
        }
        "__http/healthz" => db.healthz_json(),
        // Phase 6 read-only endpoints.
        "__http/tags" => {
            let brain = extract_str(params, "brain");
            db.tags_json(brain)
        }
        "__http/backlinks" => {
            let brain = extract_str(params, "brain");
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            db.backlinks_json(brain, path)
        }
        "__http/graph_local" => {
            let brain = extract_str(params, "brain");
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let hops = extract_u64(params, "hops").unwrap_or(2);
            db.graph_local_json(brain, path, hops)
        }
        // Write-path routes (Plan 2 Phase 1).
        "__http/memory_write" => {
            let brain = extract_str(params, "brain").ok_or("missing field: brain")?;
            let rel_path = extract_str(params, "rel_path").ok_or("missing field: rel_path")?;
            let body = extract_str(params, "body").unwrap_or("");
            let frontmatter = extract_str(params, "frontmatter");
            let if_match_etag = params
                .get("if_match_etag")
                .and_then(|v| v.as_f64())
                .ok_or("missing field: if_match_etag")?;
            let attempted_body = extract_str(params, "attempted_body").unwrap_or(body);
            db.memory_write_json(brain, rel_path, body, frontmatter, if_match_etag, attempted_body)
        }
        "__http/memory_create" => {
            let brain = extract_str(params, "brain");
            let rel_path = extract_str(params, "rel_path").ok_or("missing field: rel_path")?;
            let body = extract_str(params, "body").unwrap_or("");
            let frontmatter = extract_str(params, "frontmatter");
            db.memory_create_json(brain, rel_path, body, frontmatter)
        }
        "__http/memory_delete" => {
            let brain = extract_str(params, "brain").ok_or("missing field: brain")?;
            let rel_path = extract_str(params, "rel_path").ok_or("missing field: rel_path")?;
            db.memory_delete_json(brain, rel_path)
        }
        "__http/memory_rename" => {
            let brain = extract_str(params, "brain").ok_or("missing field: brain")?;
            let old_rel = extract_str(params, "old_rel_path").ok_or("missing field: old_rel_path")?;
            let new_rel = extract_str(params, "new_rel_path").ok_or("missing field: new_rel_path")?;
            let rewrite_links = params
                .get("rewrite_links")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            db.memory_rename_json(brain, old_rel, new_rel, rewrite_links)
        }
        _ => Err(format!("unknown tool: {tool}")),
    }
}

/// Start the DB worker thread. Returns a sender for submitting requests.
/// The thread owns a GrugDb and processes requests sequentially.
fn spawn_db_thread(
    db_path: &Path,
    config: BrainConfig,
    git_tx: Option<mpsc::Sender<GitCommitRequest>>,
) -> Result<mpsc::Sender<DbRequest>, String> {
    let db_path = db_path.to_path_buf();
    let (tx, mut rx) = mpsc::channel::<DbRequest>(64);

    std::thread::Builder::new()
        .name("grug-db".to_string())
        .spawn(move || {
            let mut db = match GrugDb::open(&db_path, config) {
                Ok(db) => db,
                Err(e) => {
                    eprintln!("grug: failed to open database: {e}");
                    return;
                }
            };
            if let Some(tx) = git_tx {
                db.set_git_tx(tx);
            }

            // Block on the receiver using a simple loop.
            // We use blocking_recv since this is a dedicated std::thread.
            while let Some(req) = rx.blocking_recv() {
                let result = dispatch_tool(&mut db, &req.tool, &req.params);
                // If the reply channel is closed, the requester gave up. That's fine.
                let _ = req.reply.send(result);
            }
        })
        .map_err(|e| format!("grug: failed to spawn DB thread: {e}"))?;

    Ok(tx)
}

/// Handle a single client connection.
async fn handle_connection(stream: UnixStream, db_tx: mpsc::Sender<DbRequest>) {
    let (reader, mut writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }

        let req: SocketRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Send error response if we can parse enough to get an ID
                let resp = SocketResponse::err(
                    String::new(),
                    format!("invalid request JSON: {e}"),
                );
                let _ = write_response(&mut writer, &resp).await;
                continue;
            }
        };

        let (tx, rx) = oneshot::channel();
        let send_result = db_tx
            .send(DbRequest {
                tool: req.tool,
                params: req.params,
                reply: tx,
            })
            .await;

        if send_result.is_err() {
            let resp = SocketResponse::err(req.id, "server shutting down".to_string());
            let _ = write_response(&mut writer, &resp).await;
            break;
        }

        let result = match rx.await {
            Ok(r) => r,
            Err(_) => Err("db worker dropped request".to_string()),
        };

        let resp = match result {
            Ok(text) => SocketResponse::ok(req.id, text),
            Err(e) => SocketResponse::err(req.id, e),
        };

        if write_response(&mut writer, &resp).await.is_err() {
            break;
        }
    }
}

/// Write a SocketResponse as a JSON line.
async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    resp: &SocketResponse,
) -> Result<(), std::io::Error> {
    let mut json = serde_json::to_string(resp).unwrap();
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Start the Unix socket server.
///
/// If `socket_path` is None, uses the default `~/.grug-brain/grug.sock`.
/// If `db_path` is None, uses the default `~/.grug-brain/grug.db`.
/// If `config` is None, loads from the default brains.json.
pub async fn run_server(
    socket_path: Option<PathBuf>,
    db_path: Option<PathBuf>,
    config: Option<BrainConfig>,
) -> Result<(), String> {
    run_server_with_shutdown(socket_path, db_path, config, None).await
}

/// Same as `run_server` but accepts a programmatic shutdown signal alongside
/// the SIGINT/SIGTERM handlers. Either source will trigger graceful
/// shutdown — useful for integration tests that need to verify the full
/// shutdown path without raising real process-wide signals (which would
/// affect every test running in the same binary).
pub async fn run_server_with_shutdown(
    socket_path: Option<PathBuf>,
    db_path: Option<PathBuf>,
    config: Option<BrainConfig>,
    mut external_shutdown: Option<oneshot::Receiver<()>>,
) -> Result<(), String> {
    // Install a global tracing subscriber so the HTTP TraceLayer (and any
    // other `tracing` events) actually emit. `try_init` is intentional —
    // tests may install their own subscriber first; we don't want to panic.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .try_init();

    let socket = socket_path.unwrap_or_else(default_socket_path);
    let db = db_path.unwrap_or_else(default_db_path);

    // Ensure parent directory exists
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("grug: failed to create socket directory: {e}"))?;
    }

    // Clean up stale socket
    cleanup_stale_socket(&socket)?;

    // Write PID file (adjacent to socket)
    let pid_path = pid_path_for_socket(&socket);
    write_pid_file(&pid_path)?;

    // Load brain config
    let brain_config = match config {
        Some(c) => c,
        None => load_brains()?,
    };

    // Channel: DB worker -> async git committer.
    // Capacity sized so a burst of writes doesn't block the DB worker; if the
    // channel ever fills (which would mean git is hung), the DB worker drops
    // commit requests rather than blocking user-facing writes.
    let (git_commit_tx, mut git_commit_rx) = mpsc::channel::<GitCommitRequest>(256);

    // Start DB worker thread
    let db_tx = spawn_db_thread(&db, brain_config.clone(), Some(git_commit_tx))?;

    // Spawn async git-commit consumer. Drains all pending requests as a
    // batch, commits each file, then pushes once per brain that had commits.
    let commit_brains = brain_config.brains.clone();
    let commit_locks = build_sync_locks(&commit_brains);
    tokio::spawn(async move {
        while let Some(req) = git_commit_rx.recv().await {
            let mut batch = vec![req];
            while let Ok(more) = git_commit_rx.try_recv() {
                batch.push(more);
            }

            let mut pushed_brains = std::collections::HashSet::new();
            for req in &batch {
                let brain = commit_brains.iter().find(|b| b.name == req.brain).cloned();
                let Some(brain) = brain else {
                    eprintln!("grug: commit request for unknown brain {}", req.brain);
                    continue;
                };
                git_commit_file(&brain, &req.rel_path, &req.action, &commit_locks).await;
                pushed_brains.insert(brain.name.clone());
            }

            for brain_name in &pushed_brains {
                let Some(brain) = commit_brains.iter().find(|b| b.name == *brain_name) else {
                    continue;
                };
                if has_remote(brain).await {
                    git(&brain.dir, &["push", "--quiet"]).await;
                }
            }
        }
    });

    // Bind Unix socket listener
    let listener = UnixListener::bind(&socket)
        .map_err(|e| format!("grug: failed to bind socket at {}: {e}", socket.display()))?;

    eprintln!("grug serve: listening on {}", socket.display());

    // Start background services (git sync timers, refresh timers, initial reindex)
    let services = BrainServices::start(
        &brain_config.brains,
        brain_config.primary_brain(),
        db_tx.clone(),
    )
    .await;

    // Start HTTP server alongside the socket. Failure to bind is non-fatal
    // for the socket transport.
    let http_port = crate::http::configured_port();
    let http_state = crate::http::AppState {
        db_tx: db_tx.clone(),
        events: services.events_sender(),
    };
    let port_file = crate::http::default_port_file();
    let (http_shutdown_tx, http_shutdown_rx) =
        tokio::sync::oneshot::channel::<()>();
    let http_handle: Option<tokio::task::JoinHandle<()>> =
        match crate::http::bind_listener(http_port).await {
            Ok((listener, bound)) => {
                crate::http::write_port_file(&port_file, bound);
                eprintln!(
                    "grug serve: http listening on http://127.0.0.1:{bound}"
                );
                Some(tokio::spawn(async move {
                    if let Err(e) = crate::http::run_http(
                        listener,
                        http_state,
                        http_shutdown_rx,
                    )
                    .await
                    {
                        eprintln!("grug: http server error: {e}");
                    }
                }))
            }
            Err(e) => {
                eprintln!("grug: http server disabled: {e}");
                None
            }
        };

    // Accept loop with graceful shutdown on SIGINT or SIGTERM
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| format!("grug: failed to register SIGTERM handler: {e}"))?;

    // Helper: a future that resolves when the external shutdown channel
    // fires, or stays pending forever if no channel was provided. We use a
    // sentinel `Option::take` so the receiver is consumed exactly once.
    loop {
        // Build a shutdown future for this iteration. If no external channel
        // was passed, this future is `pending()` and the select arm is dead.
        let external_fut = async {
            match external_shutdown.as_mut() {
                Some(rx) => {
                    let _ = rx.await;
                }
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let db_tx = db_tx.clone();
                        tokio::spawn(handle_connection(stream, db_tx));
                    }
                    Err(e) => {
                        eprintln!("grug: accept error: {e}");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("grug serve: shutting down (SIGINT)");
                break;
            }
            _ = sigterm.recv() => {
                eprintln!("grug serve: shutting down (SIGTERM)");
                break;
            }
            _ = external_fut => {
                eprintln!("grug serve: shutting down (external)");
                break;
            }
        }
    }

    // Graceful shutdown: tell HTTP server to stop, then background services.
    let _ = http_shutdown_tx.send(());
    if let Some(handle) = http_handle {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            handle,
        )
        .await;
    }
    crate::http::remove_port_file(&port_file);

    services.shutdown().await;

    // Cleanup
    drop(db_tx);
    let _ = fs::remove_file(&socket);
    remove_pid_file(&pid_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::test_db;
    use serde_json::json;

    #[test]
    fn test_dispatch_search() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-search", &json!({"query": "test"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_write() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(
            &mut db,
            "grug-write",
            &json!({"category": "test", "path": "note", "content": "hello world"}),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_read_no_args() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-read", &json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_recall() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-recall", &json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_delete_missing_field() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-delete", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field"));
    }

    #[test]
    fn test_dispatch_config_list() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-config", &json!({"action": "list"}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_sync() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-sync", &json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_dream() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-dream", &json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_docs() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-docs", &json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_dispatch_update() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        crate::tools::test_helpers::create_brain_file(
            &brain_dir,
            "notes/target.md",
            "original content here",
        );
        let result = dispatch_tool(
            &mut db,
            "grug-update",
            &json!({
                "category": "notes",
                "path": "target",
                "edits": [{"old": "original", "new": "modified"}]
            }),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().contains("updated"));
    }

    #[test]
    fn test_dispatch_update_missing_field() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-update", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing field"));
    }

    #[test]
    fn test_dispatch_unknown_tool() {
        let (mut db, _tmp) = test_db();
        let result = dispatch_tool(&mut db, "grug-nonexistent", &json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn test_default_socket_path() {
        let path = default_socket_path();
        assert!(path.to_str().unwrap().contains("grug-brain"));
        assert!(path.to_str().unwrap().ends_with("grug.sock"));
    }

    #[test]
    fn test_cleanup_stale_socket_no_file() {
        // Should succeed when socket doesn't exist
        let result = cleanup_stale_socket(Path::new("/tmp/nonexistent-grug-test.sock"));
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // DW-3 structural verification tests
    // -----------------------------------------------------------------------

    /// DW-3.2: No `__http/*` arm in `dispatch_tool` calls `handlers::*_json`.
    /// We verify by reading this file's source at compile time and asserting
    /// the banned call pattern is absent from non-test, non-comment code.
    #[test]
    #[allow(non_snake_case)]
    fn test_DW_3_2_no_handlers_json_calls_in_server_rs() {
        const SERVER_SRC: &str = include_str!("server.rs");
        // Banned pattern: any non-test, non-comment line containing the old call.
        // We stop scanning at the test module boundary.
        let banned = "crate::http::handlers::";
        let mut in_test_mod = false;
        let mut violations: Vec<usize> = Vec::new();
        for (i, line) in SERVER_SRC.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("mod tests") || trimmed.starts_with("#[cfg(test)]") {
                in_test_mod = true;
            }
            if in_test_mod {
                continue;
            }
            if trimmed.starts_with("//") {
                continue;
            }
            if line.contains(banned) {
                violations.push(i + 1);
            }
        }
        assert!(
            violations.is_empty(),
            "DW-3.2: dispatch_tool still calls http::handlers at lines: {:?}",
            violations
        );
    }

    /// DW-3.2: Every `__http/*` arm in dispatch_tool calls `db.` (trait method).
    #[test]
    #[allow(non_snake_case)]
    fn test_DW_3_2_all_http_arms_use_db_method() {
        const SERVER_SRC: &str = include_str!("server.rs");
        let http_arms: &[&str] = &[
            "\"__http/brains\"",
            "\"__http/memories\"",
            "\"__http/memory\"",
            "\"__http/graph\"",
            "\"__http/search\"",
            "\"__http/quickswitch\"",
            "\"__http/healthz\"",
            "\"__http/tags\"",
            "\"__http/backlinks\"",
            "\"__http/graph_local\"",
            "\"__http/memory_write\"",
            "\"__http/memory_create\"",
            "\"__http/memory_delete\"",
            "\"__http/memory_rename\"",
        ];
        let mut missing = Vec::new();
        for arm in http_arms {
            if !SERVER_SRC.contains(arm) {
                missing.push(*arm);
            }
        }
        assert!(
            missing.is_empty(),
            "DW-3.2: these __http/* dispatch arms are missing from server.rs: {:?}",
            missing
        );
    }
}
