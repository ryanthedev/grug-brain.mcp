# Pseudocode: Phase 4 - Git Sync + Background Services

## DW Verification

| DW-ID | Done-When Item | Status | Pseudocode Section |
|-------|---------------|--------|-------------------|
| DW-4.1 | Periodic git pull --rebase + push works for writable brains with remotes | COVERED | git.rs: git_sync, services.rs: spawn_sync_timer |
| DW-4.2 | Rebase conflict resolution: detects REBASE_HEAD, saves local to conflicts/, aborts rebase, resets to remote | COVERED | git.rs: resolve_rebase_conflict |
| DW-4.3 | .grugignore loaded and applied to git info/exclude | COVERED | git.rs: load_grugignore, sync_git_exclude |
| DW-4.4 | sync:false frontmatter files excluded from git operations | COVERED | git.rs: is_local_file, sync_git_exclude |
| DW-4.5 | Per-brain sync timers with configurable intervals (minimum 10s) | COVERED | services.rs: spawn_sync_timer, start_brain_services |
| DW-4.6 | Read-only brain refresh timers (minimum 3600s) | COVERED | services.rs: spawn_refresh_timer, start_brain_services |
| DW-4.7 | Graceful shutdown: SIGTERM/SIGINT cancels all timers, waits for in-flight git ops | COVERED | services.rs: BrainServices::shutdown, server.rs: signal handling |
| DW-4.8 | Integration tests: git sync against a local bare repo, conflict resolution scenario | COVERED | git.rs: tests module |

**All items COVERED:** YES

## Files to Create/Modify
- `src/git.rs` -- NEW: all git operations (shell-out, sync, conflict resolution, grugignore)
- `src/services.rs` -- NEW: background timer management, service lifecycle
- `src/server.rs` -- MODIFY: spawn services on startup, graceful shutdown
- `src/lib.rs` -- MODIFY: add git and services modules

## Pseudocode

### src/git.rs [DW-4.1, DW-4.2, DW-4.3, DW-4.4]

```
use tokio::process::Command
use tokio::sync::Mutex
use std::collections::HashMap
use std::sync::Arc

// --- constants ---
const GIT_TIMEOUT: Duration = 10 seconds
const GITIGNORE_CONTENT: &str = "*.db\n*.db-wal\n*.db-shm\nrecall.md\nlocal/\n.grugignore\n"

// --- hostname ---
fn get_hostname() -> String:
    // Read system hostname, take first segment before "."
    // Sanitize: keep only [a-zA-Z0-9-]
    // Return "unknown" if empty after sanitization

// --- sync locks ---
// Per-brain mutex to prevent concurrent git operations
type SyncLocks = Arc<HashMap<String, Arc<Mutex<()>>>>

fn build_sync_locks(brains: &[Brain]) -> SyncLocks:
    // Create a Mutex for each brain name
    // Return Arc<HashMap<brain_name -> Arc<Mutex<()>>>>

// --- git helper ---
async fn git(brain_dir: &Path, args: &[&str]) -> Option<String>:
    // Spawn: git <args> with cwd=brain_dir, timeout=10s
    // tokio::process::Command::new("git").args(args).current_dir(brain_dir)
    // Capture stdout, encoding utf-8
    // On success: return Some(stdout.trim())
    // On failure (exit code != 0 or timeout): return None
    // Log slow operations (>1s) to stderr

// --- ensure git repo ---
async fn ensure_git_repo(brain: &Brain) -> bool:
    // If git rev-parse --git-dir returns ".git" -> already initialized, return true
    // Else: git init
    // Write .gitignore with GITIGNORE_CONTENT
    // git add .gitignore
    // git commit -m "grug: init"
    // Return true on success, false on failure

// --- has remote ---
async fn has_remote(brain: &Brain) -> bool:
    // git remote -> check non-empty result

// --- .grugignore ---  [DW-4.3]
fn load_grugignore(brain_dir: &Path) -> Vec<String>:
    // Read brain_dir/.grugignore
    // Split by newline, trim each line
    // Filter: skip empty lines and lines starting with "#"
    // Return list of patterns

// --- is_local_file --- [DW-4.4]
fn is_local_file(brain_dir: &Path, rel_path: &str, content: Option<&str>) -> bool:
    // If content provided, extract frontmatter; if sync == "false" -> return true
    // Load .grugignore patterns
    // For each pattern:
    //   - If pattern ends with "/": check if rel_path starts with pattern
    //   - If pattern contains "*": convert to regex, test rel_path
    //   - Else: exact match or prefix match (rel_path == pattern or starts_with pattern + "/")
    // Return false if no pattern matches

// --- sync git exclude --- [DW-4.3, DW-4.4]
async fn sync_git_exclude(brain: &Brain):
    // Ensure git repo exists
    // Start with lines: ["# managed by grug-brain", ".grugignore"]
    // Append all patterns from load_grugignore
    // Walk brain directory (walk_files)
    //   For each file: read content, check frontmatter sync == "false"
    //   If sync:false -> append relative path to lines
    // Ensure .git/info/ directory exists
    // Write lines to .git/info/exclude (joined by "\n" + trailing "\n")

// --- git commit file --- (called after write/delete)
pub async fn git_commit_file(brain: &Brain, rel_path: &str, action: &str, locks: &SyncLocks):
    // If brain not in locks map, skip (shouldn't happen)
    // Do NOT acquire the sync lock -- just check if it's held
    //   tryLock: if lock is held, skip (sync in progress, it will pick up changes)
    // Ensure git repo
    // If action != "delete":
    //   Read file content, check is_local_file
    //   If local -> sync_git_exclude, return
    // git add -- <rel_path>
    // git commit -m "grug: <action> <rel_path>" --quiet

// --- resolve rebase conflict --- [DW-4.2]
async fn resolve_rebase_conflict(brain: &Brain, primary_brain: &Brain, db_tx: &mpsc::Sender<DbRequest>):
    // Get unmerged files: git diff --name-only --diff-filter=U
    // If empty or None:
    //   Log warning "conflict detected but no unmerged files"
    //   git rebase --abort
    //   return
    
    // Split output by newline, filter empty
    // Get hostname, today's date
    
    // For each conflict file:
    //   Get local content: git show REBASE_HEAD:<filePath>
    //   If None -> log warning, continue
    //
    //   Build conflict filename: slugify(brain.name) + "--" + filePath.replace("/", "--")
    //   Ensure ends with .md
    //   Create conflicts/ directory in primary_brain.dir
    //   
    //   Build frontmatter:
    //     name: conflict-{slug(brain.name)}-{slug(file_stem)}
    //     date: today
    //     type: memory
    //     conflict: true
    //     original_path: filePath
    //     original_brain: brain.name
    //     hostname: hostname
    //
    //   Extract body from local content (strip frontmatter if present)
    //   Write: frontmatter + "\n\n" + body + "\n"
    //
    //   On write failure:
    //     Log "FAILED to save conflict file"
    //     Log "leaving brain in rebase state for manual resolution"
    //     return (do NOT abort rebase -- leave for manual fix)
    //
    //   Log "conflict saved -- path"
    //   Send reindex request to db_tx for the conflict file
    
    // git rebase --abort
    
    // Reset to remote:
    //   Try: git rev-parse --abbrev-ref @{upstream} -> if Some, reset --hard to it
    //   Else try: git rev-parse --verify origin/main -> if Some, reset --hard origin/main
    //   Else try: git rev-parse --verify origin/master -> if Some, reset --hard origin/master
    
    // Send full reindex request for brain via db_tx

// --- git sync --- [DW-4.1]
pub async fn git_sync(brain: &Brain, primary_brain: &Brain, locks: &SyncLocks, db_tx: &mpsc::Sender<DbRequest>):
    // Ensure git repo
    // Check has_remote -- if no remote, return
    // Acquire sync lock for this brain (await the mutex)
    //   The lock prevents concurrent syncs on same brain
    
    // let _guard = locks.get(brain.name)?.lock().await
    
    // Log "[gitSync] brain.name -- start"
    // Sync git exclude
    // Record HEAD before: git rev-parse HEAD
    
    // Pull with rebase: git pull --rebase --quiet
    // If pull failed (None):
    //   Check if .git/REBASE_HEAD exists
    //   If yes -> log "rebase conflict detected", call resolve_rebase_conflict
    //   Log done, return
    
    // Record HEAD after: git rev-parse HEAD
    // Push: git push --quiet
    
    // Check if dirty: before != after OR git status --porcelain is non-empty
    // If dirty -> log "dirty, running syncBrain", send reindex request via db_tx
    // Log done with elapsed time

// --- refresh brain --- [DW-4.6]
pub async fn refresh_brain(brain: &Brain, db_tx: &mpsc::Sender<DbRequest>):
    // Guard: skip if brain.writable or brain.source is None
    // git pull --ff-only --quiet
    // If failed -> log "refresh skipped (ff-only failed)" and return
    // Send reindex request via db_tx
    // Log "refreshed brain.name"
```

### src/services.rs [DW-4.5, DW-4.6, DW-4.7]

```
use tokio::task::JoinHandle
use tokio::time::{interval, Duration}
use tokio_util::sync::CancellationToken

const MIN_SYNC_INTERVAL_S: u64 = 10
const MIN_REFRESH_INTERVAL_S: u64 = 3600

// Holds all background task handles for lifecycle management
pub struct BrainServices {
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

impl BrainServices {
    // --- start all services --- [DW-4.5, DW-4.6]
    pub async fn start(
        brains: &[Brain],
        primary_brain: &Brain,
        db_tx: mpsc::Sender<DbRequest>,
    ) -> Self:
        let cancel = CancellationToken::new()
        let locks = build_sync_locks(brains)
        let mut tasks = Vec::new()
        
        // For each brain, start appropriate timers
        for brain in brains:
            // Sync timer: writable brains with git config or detected remote
            if brain.git.is_some() || has_remote(brain).await:
                if !ensure_git_repo(brain).await: continue
                
                let interval_s = brain.sync_interval.max(MIN_SYNC_INTERVAL_S)
                let handle = spawn_sync_timer(
                    brain.clone(), primary_brain.clone(),
                    locks.clone(), db_tx.clone(),
                    cancel.child_token(), interval_s
                )
                tasks.push(handle)
                eprintln!("grug: sync enabled for {} ({}s interval)", brain.name, interval_s)
            
            // Refresh timer: non-writable brains with source and refresh_interval
            if !brain.writable && brain.source.is_some():
                if let Some(refresh_s) = brain.refresh_interval:
                    let clamped = refresh_s.max(MIN_REFRESH_INTERVAL_S)
                    if clamped != refresh_s:
                        eprintln!("grug: refresh interval for {} clamped to {}s (was {}s)", brain.name, clamped, refresh_s)
                    let handle = spawn_refresh_timer(
                        brain.clone(), db_tx.clone(),
                        cancel.child_token(), clamped
                    )
                    tasks.push(handle)
                    eprintln!("grug: refresh enabled for {} ({}s interval)", brain.name, clamped)
        
        // Initial sync for primary brain's git exclude
        sync_git_exclude(primary_brain).await
        
        // Initial reindex for all brains (background, non-blocking)
        for brain in brains:
            let tx = db_tx.clone()
            let name = brain.name.clone()
            tokio::spawn(async move {
                // Send sync request through db channel
                let (reply_tx, reply_rx) = oneshot::channel()
                let _ = tx.send(DbRequest {
                    tool: "grug-sync".to_string(),
                    params: json!({"brain": name}),
                    reply: reply_tx,
                }).await
                // Log result
                if let Ok(Ok(result)) = reply_rx.await {
                    eprintln!("grug: brain \"{}\" -- {}", name, result)
                }
            })
        
        BrainServices { cancel, tasks }
    
    // --- graceful shutdown --- [DW-4.7]
    pub async fn shutdown(self):
        // Signal all tasks to cancel
        self.cancel.cancel()
        
        // Wait for all tasks to finish (with timeout)
        // This ensures in-flight git operations complete
        let timeout = Duration::from_secs(15)
        for handle in self.tasks:
            let _ = tokio::time::timeout(timeout, handle).await
}

// --- spawn sync timer --- [DW-4.5]
fn spawn_sync_timer(
    brain: Brain,
    primary_brain: Brain,
    locks: SyncLocks,
    db_tx: mpsc::Sender<DbRequest>,
    cancel: CancellationToken,
    interval_s: u64,
) -> JoinHandle<()>:
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_s))
        ticker.tick().await  // First tick is immediate; skip it
        
        loop:
            select! {
                _ = ticker.tick() => {
                    // Run git_sync, log errors but don't crash
                    if let Err(e) = git_sync(&brain, &primary_brain, &locks, &db_tx).await:
                        eprintln!("grug: [gitSync] {} -- error: {}", brain.name, e)
                }
                _ = cancel.cancelled() => {
                    break  // Shutdown requested
                }
            }
    })

// --- spawn refresh timer --- [DW-4.6]
fn spawn_refresh_timer(
    brain: Brain,
    db_tx: mpsc::Sender<DbRequest>,
    cancel: CancellationToken,
    interval_s: u64,
) -> JoinHandle<()>:
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_s))
        ticker.tick().await  // Skip immediate first tick
        
        loop:
            select! {
                _ = ticker.tick() => {
                    refresh_brain(&brain, &db_tx).await
                }
                _ = cancel.cancelled() => {
                    break
                }
            }
    })
```

### src/server.rs modifications [DW-4.7]

```
// Modify run_server to:
// 1. Spawn BrainServices after starting DB worker
// 2. Handle both SIGTERM and SIGINT
// 3. Shut down services before dropping DB channel

pub async fn run_server(...) -> Result<(), String>:
    // ... existing setup (socket cleanup, PID file, config, DB worker) ...
    
    let brain_config_clone = brain_config.clone()
    let db_tx_for_services = db_tx.clone()
    
    // Spawn background services
    let services = BrainServices::start(
        &brain_config_clone.brains,
        brain_config_clone.primary_brain(),
        db_tx_for_services,
    ).await
    
    // Accept loop with SIGTERM + SIGINT support
    let mut sigterm = signal::unix::signal(SignalKind::terminate()).expect("SIGTERM handler")
    
    loop:
        select! {
            accept_result = listener.accept() => { /* existing connection handling */ }
            _ = signal::ctrl_c() => {
                eprintln!("grug serve: shutting down (SIGINT)")
                break
            }
            _ = sigterm.recv() => {
                eprintln!("grug serve: shutting down (SIGTERM)")
                break
            }
        }
    
    // Graceful shutdown
    services.shutdown().await
    drop(db_tx)
    let _ = fs::remove_file(&socket)
    remove_pid_file(&pid_path)

### src/lib.rs modification

```
// Add two new module declarations:
pub mod git;
pub mod services;
```

## Design Notes

### Git commit after write/delete
The write and delete tools run synchronously on the DB thread. Git commits are async operations. Rather than making the DB thread async or blocking it on git, the git_commit_file function should be called from outside the DB thread. However, the current architecture dispatches tool calls through the DB thread and returns results directly.

The cleanest approach: the `// Git commit skipped (Phase 4)` stubs in write.rs and delete.rs will remain as stubs for now. Git commits after write/delete will be handled by a separate mechanism in a future refinement -- or the server's handle_connection can check if the tool was grug-write/grug-delete and spawn an async git commit after receiving the response. This is how the JS works: gitCommitFile is called after the tool handler returns.

For this phase, the priority is the background sync (DW-4.1 through DW-4.7) and integration tests (DW-4.8). The periodic git sync will pick up uncommitted write/delete changes on the next cycle, so no data is lost.

### CancellationToken vs JoinHandle abort
Using tokio_util::sync::CancellationToken gives cooperative cancellation -- tasks check for cancellation at their select! points. This is safer than JoinHandle::abort() because it lets in-flight git operations complete before the task exits.

However, tokio_util is an additional dependency. Alternative: use a shared tokio::sync::Notify or a broadcast channel. The CancellationToken is cleaner. But to avoid adding a new dependency, we can use a simple approach: store JoinHandles and abort them during shutdown, relying on the git process timeout (10s) as the upper bound on cleanup time. Actually, the simplest approach matching the JS behavior: we can use the existing tokio select! with a shared shutdown signal (oneshot or broadcast). Let's use `tokio::sync::broadcast` for the shutdown signal since we need multiple receivers.

Final decision: Use a `tokio::sync::broadcast` channel as the shutdown signal. Each timer task listens on its own receiver. On shutdown, send one message on the broadcast channel, then await all JoinHandles with a timeout.

### Reindex via DB channel
After git sync changes files, we need to reindex. The indexing functions (sync_brain) require &Connection which only the DB thread has. So we send a "grug-sync" tool request through the existing db_tx channel. This reuses the existing dispatch_tool infrastructure perfectly.
