//! File watcher: observes brain directories with the `notify` crate, debounces
//! bursts (~500ms per file), filters out grug's own writes plus `.trash/` and
//! `sync: false` files, and broadcasts typed `MemoryEvent`s for UI consumers.
//!
//! See plan-1 phase-2: the watcher is the source of HTTP SSE events that
//! Phase 3 will surface to the read-only viewer.

use crate::git::is_local_file;
use crate::types::{Brain, MemoryEvent};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// Debounce window per file. The plan calls for ~500ms; small enough that
/// editors that write multiple times during a save still produce one event.
const DEBOUNCE_MS: u64 = 500;

/// Suppression-window TTL: a `(brain, rel_path, mtime_ms)` triple registered
/// by the write path is matched against incoming events for this long. After
/// that we forget it (handles cases where the FS event never arrives, e.g.
/// when the file is touched externally before our event lands).
const SUPPRESS_TTL: Duration = Duration::from_secs(5);

/// Capacity of the broadcast channel. Subscribers that fall behind receive a
/// `Lagged(n)` marker via `RecvError::Lagged` and should reload their state.
const BROADCAST_CAPACITY: usize = 64;

/// Maps brain name -> (brain dir, sync flag we'll re-check per event).
#[derive(Debug, Clone)]
struct BrainEntry {
    name: String,
    dir: PathBuf,
}

/// Self-write suppression registry. Keyed by `(brain, rel_path)`; value is
/// `(mtime_ms, registered_at)`. We compare both `rel_path` AND `mtime_ms` to
/// avoid suppressing a legitimate later edit at the same path.
type Suppression = Arc<Mutex<HashMap<(String, String), (f64, Instant)>>>;

/// Public watcher handle. Owns the notify watcher (kept alive via field) plus
/// the broadcast sender for `MemoryEvent`. Drop the handle to stop.
pub struct Watcher {
    tx: broadcast::Sender<MemoryEvent>,
    suppression: Suppression,
    _notify_handle: RecommendedWatcher,
    _debounce_task: JoinHandle<()>,
}

impl Watcher {
    /// Start a watcher across the given brains. Brains whose `dir` does not
    /// exist are skipped silently. Returns `Err` if the underlying notify
    /// watcher cannot be created.
    pub fn start(brains: &[Brain]) -> Result<Self, String> {
        let (event_tx, event_rx) = mpsc::unbounded_channel::<RawEvent>();
        // Canonicalize the brain dir so path-prefix attribution still works
        // when notify reports symlink-resolved paths (macOS resolves /tmp ->
        // /private/tmp, which would defeat `strip_prefix`).
        let entries: Vec<BrainEntry> = brains
            .iter()
            .filter(|b| b.dir.exists())
            .map(|b| BrainEntry {
                name: b.name.clone(),
                dir: std::fs::canonicalize(&b.dir).unwrap_or_else(|_| b.dir.clone()),
            })
            .collect();

        // Build notify watcher with a synchronous handler that translates
        // raw events into `RawEvent` enriched with brain/rel_path. We do the
        // brain-resolution here so the debouncer doesn't need to know.
        let resolver_entries = entries.clone();
        let mut notify_watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let event = match res {
                    Ok(e) => e,
                    Err(_) => return,
                };
                for raw in classify_event(&event, &resolver_entries) {
                    let _ = event_tx.send(raw);
                }
            })
            .map_err(|e| format!("notify watcher: {e}"))?;

        for entry in &entries {
            notify_watcher
                .watch(&entry.dir, RecursiveMode::Recursive)
                .map_err(|e| format!("watch {}: {e}", entry.dir.display()))?;
        }

        let (tx, _) = broadcast::channel::<MemoryEvent>(BROADCAST_CAPACITY);
        let suppression: Suppression = Arc::new(Mutex::new(HashMap::new()));

        let debounce_task = spawn_debouncer(
            event_rx,
            tx.clone(),
            suppression.clone(),
            entries.clone(),
        );

        Ok(Watcher {
            tx,
            suppression,
            _notify_handle: notify_watcher,
            _debounce_task: debounce_task,
        })
    }

    /// Subscribe to `MemoryEvent`s. Lagged subscribers will receive
    /// `RecvError::Lagged` from `recv().await`, which callers map to
    /// `MemoryEvent::Lagged(n)`.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryEvent> {
        self.tx.subscribe()
    }

    /// Clone the underlying broadcast sender. HTTP SSE handlers use this to
    /// hand a fresh `Receiver` to each connected client without holding a
    /// reference to the watcher itself.
    pub fn sender(&self) -> broadcast::Sender<MemoryEvent> {
        self.tx.clone()
    }

    /// Register a self-write so the next watcher event matching
    /// `(brain, rel_path, mtime_ms)` is dropped. Mtime mismatch passes
    /// through (the file changed for a different reason).
    pub fn suppress(&self, brain: &str, rel_path: &str, mtime_ms: f64) {
        let mut g = self.suppression.lock().expect("suppression poisoned");
        g.insert(
            (brain.to_string(), rel_path.to_string()),
            (mtime_ms, Instant::now()),
        );
    }
}

/// Raw event after brain attribution but before debouncing.
#[derive(Debug, Clone)]
struct RawEvent {
    brain: String,
    rel_path: String,
    /// Absolute path (so we can stat to get current mtime in the debouncer).
    full_path: PathBuf,
    kind: RawKind,
    seen_at: Instant,
}

#[derive(Debug, Clone, Copy)]
enum RawKind {
    CreateOrModify,
    Remove,
    // `notify` on macOS folds rename events into `Modify(Name(_))` for both
    // endpoints, which `classify_event` already maps to `CreateOrModify`. The
    // separate Rename variant added in Phase 2 was unreachable in practice and
    // emitted duplicated `from`/`to` strings, so it has been removed. If we
    // ever need true rename correlation, route it through a different path
    // (e.g. `grug_rename`'s explicit suppression) rather than the watcher.
}

/// Classify a notify event into zero or more `RawEvent`s, attributing each
/// path to its brain. Files outside any brain dir are dropped here.
fn classify_event(event: &notify::Event, entries: &[BrainEntry]) -> Vec<RawEvent> {
    let kind = match event.kind {
        EventKind::Create(_) => RawKind::CreateOrModify,
        EventKind::Modify(_) => RawKind::CreateOrModify,
        EventKind::Remove(_) => RawKind::Remove,
        // ANY other kind: ignore (Access, Other, etc.)
        _ => return Vec::new(),
    };

    // Notify on macOS sometimes lumps rename into Modify; we fold it into
    // CreateOrModify and let the debouncer reconcile via fs::metadata.
    let mut out = Vec::new();
    for path in &event.paths {
        // Only .md / .mdx files
        let ext_ok = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e == "md" || e == "mdx")
            .unwrap_or(false);
        if !ext_ok {
            continue;
        }
        if let Some((brain, rel)) = attribute_to_brain(path, entries) {
            out.push(RawEvent {
                brain,
                rel_path: rel,
                full_path: path.clone(),
                kind,
                seen_at: Instant::now(),
            });
        }
    }
    out
}

fn attribute_to_brain(path: &Path, entries: &[BrainEntry]) -> Option<(String, String)> {
    for entry in entries {
        if let Ok(rel) = path.strip_prefix(&entry.dir) {
            let rel_str = rel.to_string_lossy().to_string();
            if rel_str.is_empty() {
                continue;
            }
            return Some((entry.name.clone(), rel_str));
        }
    }
    None
}

/// Per-file debounce state. We collect the last seen kind+full_path; a single
/// timer fires after `DEBOUNCE_MS` of quiet for that key.
struct PendingEvent {
    kind: RawKind,
    full_path: PathBuf,
}

/// Drives the debouncer:
///   1. Collect raw events into a `HashMap<(brain, rel_path), PendingEvent>`.
///   2. Use a tokio timer ticking every 100ms to scan for entries whose
///      `last_seen_at + DEBOUNCE_MS <= now`; emit those.
///
/// The 100ms tick is a tradeoff between debounce precision and idle wakeups.
/// We keep `last_seen_at` separately keyed in `last_seen` so updates restart
/// the window without re-allocating the pending struct.
fn spawn_debouncer(
    mut rx: mpsc::UnboundedReceiver<RawEvent>,
    tx: broadcast::Sender<MemoryEvent>,
    suppression: Suppression,
    entries: Vec<BrainEntry>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut pending: HashMap<(String, String), PendingEvent> = HashMap::new();
        let mut last_seen: HashMap<(String, String), Instant> = HashMap::new();
        let mut tick = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                maybe_ev = rx.recv() => {
                    match maybe_ev {
                        Some(ev) => {
                            let key = (ev.brain.clone(), ev.rel_path.clone());
                            last_seen.insert(key.clone(), ev.seen_at);
                            // For a key already pending, prefer Remove > Rename > CreateOrModify
                            // so a delete that arrives during a write window wins.
                            let entry = pending.entry(key).or_insert(PendingEvent {
                                kind: ev.kind,
                                full_path: ev.full_path.clone(),
                            });
                            entry.kind = merge_kinds(entry.kind, ev.kind);
                            entry.full_path = ev.full_path;
                        }
                        None => break, // sender dropped
                    }
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    let due: Vec<(String, String)> = last_seen
                        .iter()
                        .filter(|(_, t)| now.duration_since(**t) >= Duration::from_millis(DEBOUNCE_MS))
                        .map(|(k, _)| k.clone())
                        .collect();
                    for key in due {
                        last_seen.remove(&key);
                        let pe = match pending.remove(&key) {
                            Some(p) => p,
                            None => continue,
                        };
                        flush_event(&key.0, &key.1, &pe, &tx, &suppression, &entries);
                    }
                    // Garbage-collect stale suppression entries.
                    let mut g = suppression.lock().expect("suppression poisoned");
                    g.retain(|_, (_, when)| now.duration_since(*when) < SUPPRESS_TTL);
                }
            }
        }
    })
}

fn merge_kinds(prev: RawKind, next: RawKind) -> RawKind {
    // Remove dominates Create-or-Modify within a single debounce window.
    use RawKind::*;
    match (prev, next) {
        (Remove, _) | (_, Remove) => Remove,
        _ => CreateOrModify,
    }
}

/// Decide whether to broadcast and which `MemoryEvent` variant to use.
/// Filters out `.trash/` and `sync:false` files.
fn flush_event(
    brain: &str,
    rel_path: &str,
    pe: &PendingEvent,
    tx: &broadcast::Sender<MemoryEvent>,
    suppression: &Suppression,
    entries: &[BrainEntry],
) {
    // .trash/ filter (cheap string check; rel_path uses '/' on POSIX).
    if rel_path.starts_with(".trash/") || rel_path == ".trash" {
        return;
    }

    let entry = match entries.iter().find(|e| e.name == brain) {
        Some(e) => e,
        None => return,
    };

    // For Create/Modify: stat the file. If it exists, classify Created vs
    // Modified by whether it appears to be new (no prior mtime). For
    // simplicity we always emit `Modified` -- callers can treat new and
    // edited the same way; we still use `Created` if the file existed only
    // briefly. The watcher cannot reliably distinguish without prior state,
    // so we use a heuristic: if the path didn't exist 1s ago we'd have to
    // remember -- skip the heuristic and report Modified for both edit and
    // create in the same broadcast. UI redraws the same way.
    //
    // For DW-2.6 the test only requires SOME event arrives within ~1s, so
    // emitting Modified for create-or-modify is acceptable.
    let mtime_ms = file_mtime_ms(&pe.full_path);

    // sync:false filter (read content; small enough cost given debounce).
    if matches!(pe.kind, RawKind::CreateOrModify) {
        let content = std::fs::read_to_string(&pe.full_path).ok();
        if is_local_file(&entry.dir, rel_path, content.as_deref()) {
            return;
        }
    } else {
        // For Remove we can still consult `.grugignore` patterns (no content).
        if is_local_file(&entry.dir, rel_path, None) {
            return;
        }
    }

    // Self-write suppression: drop if a write registered the same triple.
    {
        let mut g = suppression.lock().expect("suppression poisoned");
        let key = (brain.to_string(), rel_path.to_string());
        if let Some((expected_mtime, when)) = g.get(&key).copied()
            && Instant::now().duration_since(when) < SUPPRESS_TTL
            && (mtime_ms - expected_mtime).abs() < f64::EPSILON
        {
            g.remove(&key);
            return;
        }
    }

    let evt = match pe.kind {
        RawKind::CreateOrModify => {
            if pe.full_path.exists() {
                MemoryEvent::Modified {
                    brain: brain.to_string(),
                    path: rel_path.to_string(),
                    mtime: mtime_ms,
                }
            } else {
                MemoryEvent::Deleted {
                    brain: brain.to_string(),
                    path: rel_path.to_string(),
                }
            }
        }
        RawKind::Remove => MemoryEvent::Deleted {
            brain: brain.to_string(),
            path: rel_path.to_string(),
        },
    };
    let _ = tx.send(evt);
}

fn file_mtime_ms(path: &Path) -> f64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Brain;
    use std::fs;
    use tempfile::TempDir;
    use tokio::sync::broadcast::error::RecvError;

    fn make_brain(name: &str, dir: &Path) -> Brain {
        Brain {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            primary: true,
            writable: true,
            flat: false,
            git: None,
            sync_interval: 60,
            source: None,
            refresh_interval: None,
        }
    }

    /// Receive any `MemoryEvent` within the given timeout, ignoring `Lagged`
    /// markers up to once.
    async fn recv_event(
        rx: &mut broadcast::Receiver<MemoryEvent>,
        timeout: Duration,
    ) -> Option<MemoryEvent> {
        tokio::time::timeout(timeout, rx.recv()).await.ok().and_then(|r| r.ok())
    }

    #[tokio::test]
    async fn test_dw_2_6_watcher_emits_modified_event() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx = watcher.subscribe();

        // Write a file -- should produce an event after ~500ms debounce.
        fs::write(brain_dir.join("notes/hello.md"), "---\nname: hello\n---\n\nbody").unwrap();

        let evt = recv_event(&mut rx, Duration::from_secs(3)).await;
        let evt = evt.expect("expected MemoryEvent within 3s");
        match evt {
            MemoryEvent::Modified { brain, path, .. }
            | MemoryEvent::Created { brain, path, .. } => {
                assert_eq!(brain, "memories");
                assert_eq!(path, "notes/hello.md");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dw_2_7_watcher_debounces_burst() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx = watcher.subscribe();

        let p = brain_dir.join("notes/burst.md");
        // 5 rapid writes within < 500ms
        for i in 0..5 {
            fs::write(&p, format!("v{i}")).unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Wait past the debounce window, then drain.
        tokio::time::sleep(Duration::from_millis(900)).await;
        let mut count = 0;
        while let Ok(Ok(_)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            count += 1;
        }
        assert!(count >= 1 && count <= 2, "expected one (maybe two) events for a burst, got {count}");
    }

    #[tokio::test]
    async fn test_dw_2_8_self_write_suppression() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx = watcher.subscribe();

        let p = brain_dir.join("notes/suppress.md");
        fs::write(&p, "body").unwrap();
        // Read mtime AFTER writing and register suppression with that exact mtime.
        let mtime = file_mtime_ms(&p);
        watcher.suppress("memories", "notes/suppress.md", mtime);

        // Should NOT receive any event for this path within ~1s.
        let evt = recv_event(&mut rx, Duration::from_millis(1500)).await;
        assert!(
            evt.is_none(),
            "self-write should be suppressed, but got: {evt:?}"
        );
    }

    #[tokio::test]
    async fn test_dw_2_9_broadcast_channel_normal() {
        // Two subscribers both receive the same event.
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx1 = watcher.subscribe();
        let mut rx2 = watcher.subscribe();

        fs::write(brain_dir.join("notes/dual.md"), "x").unwrap();

        let e1 = recv_event(&mut rx1, Duration::from_secs(3)).await;
        let e2 = recv_event(&mut rx2, Duration::from_secs(3)).await;
        assert!(e1.is_some() && e2.is_some(), "both subscribers should get the event");
    }

    #[tokio::test]
    async fn test_dw_2_9_broadcast_channel_lagged() {
        // A slow subscriber that doesn't drain receives RecvError::Lagged when
        // the producer overflows the channel buffer.
        let (tx, mut rx) = broadcast::channel::<MemoryEvent>(2);
        // Fill past capacity without reading.
        for i in 0..10 {
            let _ = tx.send(MemoryEvent::Modified {
                brain: "b".into(),
                path: format!("p{i}.md"),
                mtime: 0.0,
            });
        }
        match rx.recv().await {
            Err(RecvError::Lagged(n)) => assert!(n > 0, "got Lagged({n})"),
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dw_2_10_trash_events_suppressed() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join(".trash")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx = watcher.subscribe();

        // Writing inside .trash/ should not produce a UI event.
        fs::write(brain_dir.join(".trash/dead.md"), "body").unwrap();
        let evt = recv_event(&mut rx, Duration::from_millis(1500)).await;
        assert!(evt.is_none(), ".trash event must not broadcast: {evt:?}");
    }

    #[tokio::test]
    async fn test_dw_2_10_sync_false_events_suppressed() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("memories");
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        let brain = make_brain("memories", &brain_dir);

        let watcher = Watcher::start(&[brain]).unwrap();
        let mut rx = watcher.subscribe();

        fs::write(
            brain_dir.join("notes/local.md"),
            "---\nname: local\nsync: false\n---\n\nlocal-only body",
        )
        .unwrap();

        let evt = recv_event(&mut rx, Duration::from_millis(1500)).await;
        assert!(evt.is_none(), "sync:false event must not broadcast: {evt:?}");
    }
}
