pub mod config;
pub mod conversation;
pub mod delete;
pub mod docs;
pub mod dream;
pub mod indexing;
pub mod read;
pub mod recall;
pub mod rename;
pub mod search;
pub mod similarity;
pub mod sync;
pub mod tfidf;
pub mod update;
pub mod write;

use crate::config::load_brains_from;
use crate::db::init_db;
use crate::types::{Brain, BrainConfig};
use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// A request the DB worker emits asking the async layer to commit a single file
/// to git. Fire-and-forget: failures are logged by the receiver, never returned
/// to the user.
#[derive(Debug, Clone)]
pub struct GitCommitRequest {
    pub brain: String,
    pub rel_path: String,
    /// Commit-message verb: "write" or "delete".
    pub action: String,
}

/// Per-(brain, path) lock entry.
type PathLockEntry = Arc<Mutex<()>>;

/// Per-(brain, path) mutex map. Serializes the read-mtime / check-conflict /
/// write / index sequence within a single tool call so it composes with future
/// parallel write paths (HTTP layer in plan-1 phase 3).
///
/// The DB worker is single-threaded today, so this is forward-looking insurance
/// rather than a current correctness fix. Cheap when uncontended.
#[derive(Default)]
pub struct PathLocks {
    inner: Mutex<HashMap<(String, String), PathLockEntry>>,
}

impl PathLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get (or create) the lock for a (brain, path). Outer map lock is held only
    /// for the insertion; the returned `Arc<Mutex<()>>` is what callers hold
    /// across the critical section.
    pub fn for_path(&self, brain: &str, path: &str) -> Arc<Mutex<()>> {
        let mut g = self.inner.lock().expect("PathLocks map mutex poisoned");
        g.entry((brain.to_string(), path.to_string()))
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

pub const SEARCH_PAGE_SIZE: usize = 20;
pub const BROWSE_PAGE_SIZE: usize = 100;
pub const STALE_DAYS: i64 = 90;

/// Shared database wrapper holding the SQLite connection and brain configuration.
/// All tool functions operate on this struct.
pub struct GrugDb {
    conn: Connection,
    config: BrainConfig,
    /// Per-(brain, path) write serialization. See `PathLocks`.
    path_locks: Arc<PathLocks>,
    /// Optional channel emitting `GitCommitRequest` after every successful
    /// write/delete. `None` in unit tests by default; the server wires up a
    /// real receiver in `run_server`.
    git_tx: Option<mpsc::Sender<GitCommitRequest>>,
}

impl GrugDb {
    /// Open (or create) the grug database and load brain config.
    pub fn open(db_path: &Path, config: BrainConfig) -> Result<Self, String> {
        let conn = init_db(db_path).map_err(|e| format!("failed to open database: {e}"))?;
        Ok(Self {
            conn,
            config,
            path_locks: Arc::new(PathLocks::new()),
            git_tx: None,
        })
    }

    /// Install the channel that downstream git-commit work flows through.
    /// Called once by `run_server` after spawning the receiver task.
    pub fn set_git_tx(&mut self, tx: mpsc::Sender<GitCommitRequest>) {
        self.git_tx = Some(tx);
    }

    pub fn path_locks(&self) -> Arc<PathLocks> {
        self.path_locks.clone()
    }

    /// Try to enqueue a git-commit request. Best-effort: if the channel is full
    /// or absent (tests), this is a no-op. The user-facing reply does not depend
    /// on it.
    pub fn enqueue_git_commit(&self, brain: &str, rel_path: &str, action: &str) {
        if let Some(tx) = &self.git_tx {
            let req = GitCommitRequest {
                brain: brain.to_string(),
                rel_path: rel_path.to_string(),
                action: action.to_string(),
            };
            // try_send: never block the DB worker
            let _ = tx.try_send(req);
        }
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn config(&self) -> &BrainConfig {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut BrainConfig {
        &mut self.config
    }

    /// Check brains.json mtime and reload config if it has changed.
    pub fn maybe_reload_config(&mut self) {
        let config_path = &self.config.config_path;
        let current_mtime = match fs::metadata(config_path) {
            Ok(meta) => {
                meta.modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
            }
            Err(_) => return, // Config file gone; keep current brains
        };

        if current_mtime == self.config.last_mtime {
            return; // File unchanged
        }

        match load_brains_from(Some(config_path)) {
            Ok(mut new_config) => {
                new_config.last_mtime = current_mtime;
                self.config = new_config;
            }
            Err(_) => {
                // Keep current brains on parse error (matching JS behavior)
            }
        }
    }

    /// Resolve a brain by name, defaulting to the primary brain.
    pub fn resolve_brain(&self, name: Option<&str>) -> Result<&Brain, String> {
        match name {
            None => Ok(self.config.primary_brain()),
            Some(n) => self
                .config
                .get(n)
                .ok_or_else(|| format!("unknown brain \"{n}\"")),
        }
    }
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;
    use crate::types::Brain;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create a GrugDb backed by a temp directory and an open git-commit
    /// channel. Returns the receiver so tests can assert on emitted requests.
    pub fn test_db_with_git() -> (GrugDb, TempDir, mpsc::Receiver<GitCommitRequest>) {
        let (mut db, tmp) = test_db();
        let (tx, rx) = mpsc::channel::<GitCommitRequest>(16);
        db.set_git_tx(tx);
        (db, tmp, rx)
    }

    /// Create a GrugDb backed by a temp directory with a single primary brain.
    pub fn test_db() -> (GrugDb, TempDir) {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(&brain_dir).unwrap();

        let config = BrainConfig {
            brains: vec![Brain {
                name: "memories".to_string(),
                dir: brain_dir,
                primary: true,
                writable: true,
                flat: false,
                git: None,
                sync_interval: 60,
                source: None,
                refresh_interval: None,
            }],
            primary: "memories".to_string(),
            config_path: tmp.path().join("brains.json"),
            last_mtime: None,
        };

        let db_path = tmp.path().join("grug.db");
        let db = GrugDb::open(&db_path, config).unwrap();
        (db, tmp)
    }

    /// Create a test DB with an additional non-primary brain.
    pub fn test_db_multi() -> (GrugDb, TempDir) {
        let tmp = TempDir::new().unwrap();
        let primary_dir = tmp.path().join("memories");
        let docs_dir = tmp.path().join("docs");
        fs::create_dir_all(&primary_dir).unwrap();
        fs::create_dir_all(&docs_dir).unwrap();

        let config = BrainConfig {
            brains: vec![
                Brain {
                    name: "memories".to_string(),
                    dir: primary_dir,
                    primary: true,
                    writable: true,
                    flat: false,
                    git: None,
                    sync_interval: 60,
                    source: None,
                    refresh_interval: None,
                },
                Brain {
                    name: "docs".to_string(),
                    dir: docs_dir,
                    primary: false,
                    writable: false,
                    flat: true,
                    git: None,
                    sync_interval: 60,
                    source: Some("github:org/repo".to_string()),
                    refresh_interval: Some(3600),
                },
            ],
            primary: "memories".to_string(),
            config_path: tmp.path().join("brains.json"),
            last_mtime: None,
        };

        let db_path = tmp.path().join("grug.db");
        let db = GrugDb::open(&db_path, config).unwrap();
        (db, tmp)
    }

    /// Helper: create a file in a brain directory.
    pub fn create_brain_file(brain_dir: &Path, rel_path: &str, content: &str) -> PathBuf {
        let full = brain_dir.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        full
    }
}
