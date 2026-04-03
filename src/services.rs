use crate::git::{
    build_sync_locks, ensure_git_repo, git_sync, has_remote, refresh_brain, sync_git_exclude,
    SyncLocks,
};
use crate::server::DbRequest;
use crate::types::Brain;
use serde_json::json;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

/// Minimum sync interval in seconds (prevents hammering git).
const MIN_SYNC_INTERVAL_S: u64 = 10;

/// Minimum refresh interval in seconds (1 hour).
const MIN_REFRESH_INTERVAL_S: u64 = 3600;

/// Manages all background services (sync timers, refresh timers).
/// Provides lifecycle management: start all, shut down all.
pub struct BrainServices {
    tasks: Vec<JoinHandle<()>>,
    /// Sender used to broadcast shutdown. When dropped, all receivers get a RecvError,
    /// which we use as the shutdown signal.
    _shutdown_tx: broadcast::Sender<()>,
}

impl BrainServices {
    /// Start background services for all brains.
    ///
    /// For writable brains with remotes: starts a periodic git sync timer.
    /// For read-only brains with source + refresh_interval: starts a periodic refresh timer.
    /// Also performs initial sync of git excludes and background reindexing.
    pub async fn start(
        brains: &[Brain],
        primary_brain: &Brain,
        db_tx: mpsc::Sender<DbRequest>,
    ) -> Self {
        let locks = build_sync_locks(brains);
        let mut tasks = Vec::new();

        // Shutdown channel: when _shutdown_tx is dropped, all receivers will get RecvError
        let (_shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);

        for brain in brains {
            // Sync timer for brains with git remotes
            if brain.git.is_some() || has_remote(brain).await {
                if !ensure_git_repo(brain).await {
                    continue;
                }

                let interval_s = brain.sync_interval.max(MIN_SYNC_INTERVAL_S);
                let handle = spawn_sync_timer(
                    brain.clone(),
                    primary_brain.clone(),
                    locks.clone(),
                    db_tx.clone(),
                    _shutdown_tx.subscribe(),
                    interval_s,
                );
                tasks.push(handle);
                eprintln!(
                    "grug: sync enabled for {} ({}s interval)",
                    brain.name, interval_s
                );
            }

            // Refresh timer for read-only brains with source
            if !brain.writable && brain.source.is_some() {
                if let Some(refresh_s) = brain.refresh_interval {
                    let clamped = refresh_s.max(MIN_REFRESH_INTERVAL_S);
                    if clamped != refresh_s {
                        eprintln!(
                            "grug: refresh interval for {} clamped to {}s (was {}s)",
                            brain.name, clamped, refresh_s
                        );
                    }
                    let handle = spawn_refresh_timer(
                        brain.clone(),
                        db_tx.clone(),
                        _shutdown_tx.subscribe(),
                        clamped,
                    );
                    tasks.push(handle);
                    eprintln!(
                        "grug: refresh enabled for {} ({}s interval)",
                        brain.name, clamped
                    );
                }
            }
        }

        // Sync git exclude for primary brain
        sync_git_exclude(primary_brain).await;

        // Initial reindex for all brains (non-blocking)
        for brain in brains {
            let tx = db_tx.clone();
            let name = brain.name.clone();
            tokio::spawn(async move {
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx
                    .send(DbRequest {
                        tool: "grug-sync".to_string(),
                        params: json!({"brain": name}),
                        reply: reply_tx,
                    })
                    .await;
                if let Ok(Ok(result)) = reply_rx.await {
                    eprintln!("grug: brain \"{name}\" -- {result}");
                }
            });
        }

        BrainServices {
            tasks,
            _shutdown_tx,
        }
    }

    /// Gracefully shut down all background services.
    /// Signals all tasks to stop and waits for in-flight operations to complete.
    pub async fn shutdown(self) {
        // Drop the shutdown sender -- all receivers will get an error on recv,
        // breaking their loops
        drop(self._shutdown_tx);

        // Wait for all tasks with a timeout
        let timeout = Duration::from_secs(15);
        for handle in self.tasks {
            let _ = tokio::time::timeout(timeout, handle).await;
        }
    }
}

/// Spawn a periodic git sync timer for a writable brain.
fn spawn_sync_timer(
    brain: Brain,
    primary_brain: Brain,
    locks: SyncLocks,
    db_tx: mpsc::Sender<DbRequest>,
    mut shutdown_rx: broadcast::Receiver<()>,
    interval_s: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s));
        ticker.tick().await; // Skip the immediate first tick

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    git_sync(&brain, &primary_brain, &locks, &db_tx).await;
                }
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    })
}

/// Spawn a periodic refresh timer for a read-only brain.
fn spawn_refresh_timer(
    brain: Brain,
    db_tx: mpsc::Sender<DbRequest>,
    mut shutdown_rx: broadcast::Receiver<()>,
    interval_s: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s));
        ticker.tick().await; // Skip the immediate first tick

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    refresh_brain(&brain, &db_tx).await;
                }
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Brain;
    use std::fs;
    use tempfile::TempDir;

    fn make_brain(name: &str, dir: &std::path::Path, writable: bool) -> Brain {
        Brain {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            primary: name == "primary",
            writable,
            flat: false,
            git: None,
            sync_interval: 60,
            source: if writable {
                None
            } else {
                Some("test".to_string())
            },
            refresh_interval: if writable { None } else { Some(3600) },
        }
    }

    #[tokio::test]
    async fn test_start_and_shutdown_no_brains() {
        let (db_tx, _db_rx) = mpsc::channel::<DbRequest>(16);
        let primary_dir = TempDir::new().unwrap();
        let primary = make_brain("primary", primary_dir.path(), true);

        let services = BrainServices::start(&[], &primary, db_tx).await;
        // Should shut down cleanly with no tasks
        services.shutdown().await;
    }

    #[tokio::test]
    async fn test_start_and_shutdown_with_brains() {
        let tmp = TempDir::new().unwrap();
        let primary_dir = tmp.path().join("primary");
        fs::create_dir_all(&primary_dir).unwrap();

        let brain = make_brain("primary", &primary_dir, true);

        let (db_tx, mut db_rx) = mpsc::channel::<DbRequest>(16);

        // Drain DB requests
        tokio::spawn(async move {
            while let Some(req) = db_rx.recv().await {
                let _ = req.reply.send(Ok("0 files".to_string()));
            }
        });

        let services = BrainServices::start(&[brain.clone()], &brain, db_tx).await;

        // Give initial reindex a moment
        tokio::time::sleep(Duration::from_millis(50)).await;

        services.shutdown().await;
    }

    #[test]
    fn test_min_intervals() {
        assert_eq!(MIN_SYNC_INTERVAL_S, 10);
        assert_eq!(MIN_REFRESH_INTERVAL_S, 3600);
    }
}
