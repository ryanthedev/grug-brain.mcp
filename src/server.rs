use crate::config::{expand_home, load_brains};
use crate::protocol::{SocketRequest, SocketResponse};
use crate::services::BrainServices;
use crate::tools::GrugDb;
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
/// Default database path: ~/.grug-brain/grug.db
fn default_db_path() -> PathBuf {
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
            Ok(crate::tools::search::grug_search(db, query, page))
        }
        "grug-write" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let content = extract_str(params, "content").ok_or("missing field: content")?;
            let brain = extract_str(params, "brain");
            crate::tools::write::grug_write(db, category, path, content, brain)
        }
        "grug-read" => {
            let brain = extract_str(params, "brain");
            let category = extract_str(params, "category");
            let path = extract_str(params, "path");
            crate::tools::read::grug_read(db, brain, category, path)
        }
        "grug-recall" => {
            let category = extract_str(params, "category");
            let brain = extract_str(params, "brain");
            crate::tools::recall::grug_recall(db, category, brain)
        }
        "grug-delete" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let brain = extract_str(params, "brain");
            crate::tools::delete::grug_delete(db, category, path, brain)
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
            crate::tools::config::grug_config(
                db,
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
            crate::tools::sync::grug_sync(db, brain)
        }
        "grug-dream" => crate::tools::dream::grug_dream(db),
        "grug-update" => {
            let category = extract_str(params, "category").ok_or("missing field: category")?;
            let path = extract_str(params, "path").ok_or("missing field: path")?;
            let brain = extract_str(params, "brain");
            let edits: Vec<crate::tools::update::EditEntry> = serde_json::from_value(
                params
                    .get("edits")
                    .cloned()
                    .ok_or("missing field: edits")?,
            )
            .map_err(|e| format!("invalid edits: {e}"))?;
            crate::tools::update::grug_update(db, category, path, &edits, brain)
        }
        "grug-docs" => {
            let category = extract_str(params, "category");
            let path = extract_str(params, "path");
            let page = extract_u64(params, "page").map(|p| p as usize);
            crate::tools::docs::grug_docs(db, category, path, page)
        }
        _ => Err(format!("unknown tool: {tool}")),
    }
}

/// Start the DB worker thread. Returns a sender for submitting requests.
/// The thread owns a GrugDb and processes requests sequentially.
fn spawn_db_thread(
    db_path: &Path,
    config: BrainConfig,
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

    // Start DB worker thread
    let db_tx = spawn_db_thread(&db, brain_config.clone())?;

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

    // Accept loop with graceful shutdown on SIGINT or SIGTERM
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| format!("grug: failed to register SIGTERM handler: {e}"))?;

    loop {
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
        }
    }

    // Graceful shutdown: stop background services first
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
}
