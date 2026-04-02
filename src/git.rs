use crate::helpers::{slugify, today};
use crate::parsing::{extract_body, extract_frontmatter};
use crate::server::DbRequest;
use crate::types::Brain;
use crate::walker::walk_files;
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, Mutex};

/// Timeout for git commands (10 seconds, matching JS).
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Content for .gitignore in newly initialized repos.
const GITIGNORE_CONTENT: &str = "*.db\n*.db-wal\n*.db-shm\nrecall.md\nlocal/\n.grugignore\n";

/// Per-brain mutex map preventing concurrent git operations on the same brain.
pub type SyncLocks = Arc<HashMap<String, Arc<Mutex<()>>>>;

/// Build a sync lock map from a list of brains.
pub fn build_sync_locks(brains: &[Brain]) -> SyncLocks {
    let mut map = HashMap::new();
    for brain in brains {
        map.insert(brain.name.clone(), Arc::new(Mutex::new(())));
    }
    Arc::new(map)
}

/// Get the sanitized hostname (first segment, alphanumeric + hyphens only).
pub fn get_hostname() -> String {
    let full = hostname::get()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let first = full.split('.').next().unwrap_or("");
    let sanitized: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Run a git command in the given directory with a timeout.
/// Returns Some(stdout) on success, None on failure.
pub async fn git(brain_dir: &Path, args: &[&str]) -> Option<String> {
    let t0 = Instant::now();
    let result = tokio::time::timeout(
        GIT_TIMEOUT,
        Command::new("git")
            .args(args)
            .current_dir(brain_dir)
            .output(),
    )
    .await;

    let elapsed = t0.elapsed();
    if elapsed > Duration::from_secs(1) {
        eprintln!(
            "grug: [git] {} -- slow {}ms",
            brain_dir.display(),
            elapsed.as_millis()
        );
    }

    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Some(stdout)
        }
        Ok(Ok(_)) => {
            // Non-zero exit code
            if elapsed > Duration::from_secs(1) {
                eprintln!(
                    "grug: [git] {} -- failed {}ms",
                    brain_dir.display(),
                    elapsed.as_millis()
                );
            }
            None
        }
        Ok(Err(_)) | Err(_) => {
            // IO error or timeout
            if elapsed > Duration::from_secs(1) {
                eprintln!(
                    "grug: [git] {} -- failed {}ms",
                    brain_dir.display(),
                    elapsed.as_millis()
                );
            }
            None
        }
    }
}

/// Ensure a brain directory is a git repository. Initializes if needed.
pub async fn ensure_git_repo(brain: &Brain) -> bool {
    if let Some(result) = git(&brain.dir, &["rev-parse", "--git-dir"]).await {
        if result == ".git" {
            return true;
        }
    }
    if git(&brain.dir, &["init"]).await.is_none() {
        return false;
    }
    let gitignore_path = brain.dir.join(".gitignore");
    if fs::write(&gitignore_path, GITIGNORE_CONTENT).is_err() {
        return false;
    }
    if git(&brain.dir, &["add", ".gitignore"]).await.is_none() {
        return false;
    }
    git(&brain.dir, &["commit", "-m", "grug: init"])
        .await
        .is_some()
}

/// Check if a brain has any git remotes configured.
pub async fn has_remote(brain: &Brain) -> bool {
    match git(&brain.dir, &["remote"]).await {
        Some(output) => !output.is_empty(),
        None => false,
    }
}

/// Load .grugignore patterns from a brain directory.
pub fn load_grugignore(brain_dir: &Path) -> Vec<String> {
    let path = brain_dir.join(".grugignore");
    match fs::read_to_string(&path) {
        Ok(content) => content
            .split('\n')
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Check if a file should be excluded from git operations.
/// Returns true if the file matches a .grugignore pattern or has sync:false frontmatter.
pub fn is_local_file(brain_dir: &Path, rel_path: &str, content: Option<&str>) -> bool {
    // Check sync:false frontmatter
    if let Some(c) = content {
        let fm = extract_frontmatter(c);
        if fm.get("sync").map(|v| v.as_str()) == Some("false") {
            return true;
        }
    }

    // Check .grugignore patterns
    let patterns = load_grugignore(brain_dir);
    for pattern in &patterns {
        if pattern.ends_with('/') && rel_path.starts_with(pattern.as_str()) {
            return true;
        }
        if pattern.contains('*') {
            let escaped = regex::escape(pattern)
                .replace(r"\*", ".*");
            if let Ok(re) = Regex::new(&format!("^{escaped}$")) {
                if re.is_match(rel_path) {
                    return true;
                }
            }
        }
        if rel_path == pattern || rel_path.starts_with(&format!("{pattern}/")) {
            return true;
        }
    }
    false
}

/// Synchronize .git/info/exclude with .grugignore patterns and sync:false files.
pub async fn sync_git_exclude(brain: &Brain) {
    if !ensure_git_repo(brain).await {
        return;
    }

    let mut lines = vec![
        "# managed by grug-brain".to_string(),
        ".grugignore".to_string(),
    ];

    // Add all .grugignore patterns
    lines.extend(load_grugignore(&brain.dir));

    // Walk brain directory to find sync:false files
    for full_path in walk_files(&brain.dir) {
        if let Ok(content) = fs::read_to_string(&full_path) {
            let fm = extract_frontmatter(&content);
            if fm.get("sync").map(|v| v.as_str()) == Some("false") {
                if let Ok(rel) = full_path.strip_prefix(&brain.dir) {
                    lines.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }

    // Write exclude file
    let info_dir = brain.dir.join(".git").join("info");
    fs::create_dir_all(&info_dir).ok();
    let exclude_path = info_dir.join("exclude");
    let content = lines.join("\n") + "\n";
    fs::write(&exclude_path, content).ok();
}

/// Commit a single file change to git (called after write/delete).
/// Skips if a sync lock is held (sync will pick up changes).
pub async fn git_commit_file(brain: &Brain, rel_path: &str, action: &str, locks: &SyncLocks) {
    // Check if sync lock is held -- if so, skip (sync will commit)
    if let Some(lock) = locks.get(&brain.name) {
        if lock.try_lock().is_err() {
            return; // Sync in progress, it will pick up changes
        }
        // Lock acquired and immediately dropped -- we're clear to proceed
    }

    if !ensure_git_repo(brain).await {
        return;
    }

    if action != "delete" {
        let full_path = brain.dir.join(rel_path);
        if let Ok(content) = fs::read_to_string(&full_path) {
            if is_local_file(&brain.dir, rel_path, Some(&content)) {
                sync_git_exclude(brain).await;
                return;
            }
        }
    }

    git(&brain.dir, &["add", "--", rel_path]).await;
    git(
        &brain.dir,
        &["commit", "-m", &format!("grug: {action} {rel_path}"), "--quiet"],
    )
    .await;
}

/// Resolve a rebase conflict by saving local versions to conflicts/ and resetting.
pub async fn resolve_rebase_conflict(
    brain: &Brain,
    primary_brain: &Brain,
    db_tx: &mpsc::Sender<DbRequest>,
) {
    let unmerged_output = git(&brain.dir, &["diff", "--name-only", "--diff-filter=U"]).await;

    let conflict_files: Vec<String> = match &unmerged_output {
        Some(output) if !output.is_empty() => {
            output.split('\n').filter(|s| !s.is_empty()).map(String::from).collect()
        }
        _ => {
            eprintln!(
                "grug: conflict detected in {} but no unmerged files found",
                brain.name
            );
            git(&brain.dir, &["rebase", "--abort"]).await;
            return;
        }
    };

    let host = get_hostname();
    let date_str = today();

    for file_path in &conflict_files {
        let local_content =
            git(&brain.dir, &["show", &format!("REBASE_HEAD:{file_path}")]).await;
        let local_content = match local_content {
            Some(c) => c,
            None => {
                eprintln!(
                    "grug: could not retrieve local version of {} in {}",
                    file_path, brain.name
                );
                continue;
            }
        };

        // Build conflict filename: slugify(brain_name)--path--with--slashes.md
        let mut conflict_file_name =
            format!("{}--{}", slugify(&brain.name), file_path.replace('/', "--"));
        if !conflict_file_name.ends_with(".md") {
            conflict_file_name.push_str(".md");
        }

        let conflict_dir = primary_brain.dir.join("conflicts");
        fs::create_dir_all(&conflict_dir).ok();
        let conflict_full_path = conflict_dir.join(&conflict_file_name);

        // Build frontmatter
        let file_stem = Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_path);
        let name_slug = slugify(file_stem);

        let frontmatter = format!(
            "---\nname: conflict-{}-{}\ndate: {}\ntype: memory\nconflict: true\noriginal_path: {}\noriginal_brain: {}\nhostname: {}\n---",
            slugify(&brain.name),
            name_slug,
            date_str,
            file_path,
            brain.name,
            host,
        );

        // Extract body from local content (strip frontmatter if present)
        let body = if local_content.starts_with("---\n") {
            extract_body(&local_content)
        } else {
            local_content.clone()
        };

        let file_content = format!("{frontmatter}\n\n{body}\n");

        match fs::write(&conflict_full_path, &file_content) {
            Ok(()) => {
                eprintln!("grug: conflict saved -- {}", conflict_full_path.display());
                // Send index request for the conflict file
                // Fire-and-forget index request
                let (reply_tx, _reply_rx) = oneshot::channel();
                let _ = db_tx
                    .send(DbRequest {
                        tool: "grug-sync".to_string(),
                        params: json!({"brain": primary_brain.name}),
                        reply: reply_tx,
                    })
                    .await;
            }
            Err(e) => {
                eprintln!(
                    "grug: FAILED to save conflict file for {}: {}",
                    file_path, e
                );
                eprintln!(
                    "grug: leaving {} in rebase state for manual resolution",
                    brain.name
                );
                return; // Do NOT abort rebase
            }
        }
    }

    // Abort the rebase
    git(&brain.dir, &["rebase", "--abort"]).await;

    // Reset to remote
    let upstream = git(
        &brain.dir,
        &["rev-parse", "--abbrev-ref", "@{upstream}"],
    )
    .await;
    if let Some(ref branch) = upstream {
        git(&brain.dir, &["reset", "--hard", branch]).await;
    } else {
        let main_ref = git(&brain.dir, &["rev-parse", "--verify", "origin/main"]).await;
        if main_ref.is_some() {
            git(&brain.dir, &["reset", "--hard", "origin/main"]).await;
        } else {
            let master_ref =
                git(&brain.dir, &["rev-parse", "--verify", "origin/master"]).await;
            if master_ref.is_some() {
                git(&brain.dir, &["reset", "--hard", "origin/master"]).await;
            }
        }
    }

    // Reindex the brain
    let (reply_tx, _reply_rx) = oneshot::channel();
    let _ = db_tx
        .send(DbRequest {
            tool: "grug-sync".to_string(),
            params: json!({"brain": brain.name}),
            reply: reply_tx,
        })
        .await;
}

/// Perform a full git sync cycle for a brain: pull --rebase, push, reindex if changed.
pub async fn git_sync(
    brain: &Brain,
    primary_brain: &Brain,
    locks: &SyncLocks,
    db_tx: &mpsc::Sender<DbRequest>,
) {
    if !ensure_git_repo(brain).await {
        return;
    }
    if !has_remote(brain).await {
        return;
    }

    // Acquire per-brain sync lock
    let lock = match locks.get(&brain.name) {
        Some(l) => l.clone(),
        None => return,
    };
    let _guard = lock.lock().await;

    let t0 = Instant::now();
    eprintln!("grug: [gitSync] {} -- start", brain.name);

    sync_git_exclude(brain).await;
    let before = git(&brain.dir, &["rev-parse", "HEAD"]).await;

    let pull_result = git(&brain.dir, &["pull", "--rebase", "--quiet"]).await;

    if pull_result.is_none() {
        // Check for rebase conflict
        let rebase_head = brain.dir.join(".git").join("REBASE_HEAD");
        if rebase_head.exists() {
            eprintln!("grug: rebase conflict detected in {}", brain.name);
            resolve_rebase_conflict(brain, primary_brain, db_tx).await;
        }
        eprintln!(
            "grug: [gitSync] {} -- done (pull failed) {}ms",
            brain.name,
            t0.elapsed().as_millis()
        );
        return;
    }

    let after = git(&brain.dir, &["rev-parse", "HEAD"]).await;
    git(&brain.dir, &["push", "--quiet"]).await;

    // Reindex if remote changed or local has uncommitted changes
    let status = git(&brain.dir, &["status", "--porcelain"]).await;
    let dirty =
        before != after || status.as_deref().map(|s| !s.is_empty()).unwrap_or(false);

    if dirty {
        eprintln!("grug: [gitSync] {} -- dirty, running syncBrain", brain.name);
        let (reply_tx, _reply_rx) = oneshot::channel();
        let _ = db_tx
            .send(DbRequest {
                tool: "grug-sync".to_string(),
                params: json!({"brain": brain.name}),
                reply: reply_tx,
            })
            .await;
    }

    eprintln!(
        "grug: [gitSync] {} -- done {}ms",
        brain.name,
        t0.elapsed().as_millis()
    );
}

/// Refresh a read-only brain by pulling latest changes (ff-only).
pub async fn refresh_brain(brain: &Brain, db_tx: &mpsc::Sender<DbRequest>) {
    if brain.writable {
        return;
    }
    if brain.source.is_none() {
        return;
    }

    let result = git(&brain.dir, &["pull", "--ff-only", "--quiet"]).await;
    if result.is_none() {
        eprintln!(
            "grug: refresh skipped for {} (ff-only failed -- upstream may have rebased)",
            brain.name
        );
        return;
    }

    let (reply_tx, _reply_rx) = oneshot::channel();
    let _ = db_tx
        .send(DbRequest {
            tool: "grug-sync".to_string(),
            params: json!({"brain": brain.name}),
            reply: reply_tx,
        })
        .await;
    eprintln!("grug: refreshed {}", brain.name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Create a bare git repo that can serve as a remote.
    fn create_bare_repo(tmp: &Path) -> std::path::PathBuf {
        let bare = tmp.join("remote.git");
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .arg(&bare)
            .output()
            .expect("git init --bare");
        bare
    }

    /// Create a working git repo cloned from a bare repo.
    fn clone_repo(bare: &Path, dest: &Path) {
        std::process::Command::new("git")
            .args(["clone", &bare.to_string_lossy(), &dest.to_string_lossy()])
            .output()
            .expect("git clone");
        // Configure user for commits
        std::process::Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dest)
            .output()
            .expect("git config email");
        std::process::Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dest)
            .output()
            .expect("git config name");
    }

    fn make_brain(name: &str, dir: &Path, writable: bool) -> Brain {
        Brain {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            primary: false,
            writable,
            flat: false,
            git: Some("origin".to_string()),
            sync_interval: 60,
            source: if writable {
                None
            } else {
                Some("test".to_string())
            },
            refresh_interval: if writable { None } else { Some(3600) },
        }
    }

    // --- Unit tests for helpers ---

    #[test]
    fn test_get_hostname() {
        let host = get_hostname();
        assert!(!host.is_empty());
        // Should only contain alphanumeric and hyphens
        assert!(host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn test_load_grugignore_missing() {
        let tmp = TempDir::new().unwrap();
        let patterns = load_grugignore(tmp.path());
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_load_grugignore_basic() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(".grugignore"),
            "local/\n# comment\n\npattern*.md\n",
        )
        .unwrap();
        let patterns = load_grugignore(tmp.path());
        assert_eq!(patterns, vec!["local/", "pattern*.md"]);
    }

    #[test]
    fn test_is_local_file_sync_false() {
        let tmp = TempDir::new().unwrap();
        let content = "---\nsync: false\n---\nBody";
        assert!(is_local_file(tmp.path(), "test.md", Some(content)));
    }

    #[test]
    fn test_is_local_file_sync_true() {
        let tmp = TempDir::new().unwrap();
        let content = "---\nsync: true\n---\nBody";
        assert!(!is_local_file(tmp.path(), "test.md", Some(content)));
    }

    #[test]
    fn test_is_local_file_grugignore_dir() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".grugignore"), "local/\n").unwrap();
        assert!(is_local_file(tmp.path(), "local/secret.md", None));
        assert!(!is_local_file(tmp.path(), "notes/test.md", None));
    }

    #[test]
    fn test_is_local_file_grugignore_glob() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".grugignore"), "*.secret.md\n").unwrap();
        assert!(is_local_file(tmp.path(), "my.secret.md", None));
        assert!(!is_local_file(tmp.path(), "normal.md", None));
    }

    #[test]
    fn test_is_local_file_grugignore_exact() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".grugignore"), "specific.md\n").unwrap();
        assert!(is_local_file(tmp.path(), "specific.md", None));
        assert!(!is_local_file(tmp.path(), "other.md", None));
    }

    #[test]
    fn test_is_local_file_grugignore_prefix() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(".grugignore"), "drafts\n").unwrap();
        assert!(is_local_file(tmp.path(), "drafts/note.md", None));
        assert!(is_local_file(tmp.path(), "drafts", None));
        assert!(!is_local_file(tmp.path(), "notes/draft.md", None));
    }

    #[test]
    fn test_build_sync_locks() {
        let brains = vec![
            make_brain("a", Path::new("/tmp/a"), true),
            make_brain("b", Path::new("/tmp/b"), true),
        ];
        let locks = build_sync_locks(&brains);
        assert!(locks.contains_key("a"));
        assert!(locks.contains_key("b"));
    }

    // --- Integration tests with real git repos ---

    #[tokio::test]
    async fn test_ensure_git_repo_and_has_remote() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true);

        // Configure git user for the test
        std::process::Command::new("git")
            .args(["config", "--global", "init.defaultBranch", "main"])
            .output()
            .ok();

        assert!(ensure_git_repo(&brain).await);
        // Should be idempotent
        assert!(ensure_git_repo(&brain).await);

        // .gitignore should exist
        let gitignore = brain_dir.join(".gitignore");
        assert!(gitignore.exists());
        let content = fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("*.db"));

        // No remote yet
        assert!(!has_remote(&brain).await);
    }

    #[tokio::test]
    async fn test_git_sync_with_local_bare_repo() {
        let tmp = TempDir::new().unwrap();
        let bare = create_bare_repo(tmp.path());

        // Create initial commit in bare repo via a temp clone
        let init_dir = tmp.path().join("init");
        clone_repo(&bare, &init_dir);
        fs::write(init_dir.join("seed.md"), "---\nname: seed\n---\n\nSeed content\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&init_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&init_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(&init_dir)
            .output()
            .unwrap();

        // Clone the brain working copy
        let brain_dir = tmp.path().join("brain");
        clone_repo(&bare, &brain_dir);

        let brain = make_brain("test", &brain_dir, true);
        let primary = make_brain("test", &brain_dir, true);
        let locks = build_sync_locks(&[brain.clone()]);

        // Create a DB channel (we won't process reindex requests in this test)
        let (db_tx, mut db_rx) = mpsc::channel::<DbRequest>(16);

        // Drain DB requests in background
        tokio::spawn(async move {
            while let Some(req) = db_rx.recv().await {
                let _ = req.reply.send(Ok("ok".to_string()));
            }
        });

        // Add a local file and commit
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        fs::write(
            brain_dir.join("notes/local.md"),
            "---\nname: local\n---\n\nLocal content\n",
        )
        .unwrap();
        git(&brain_dir, &["add", "."]).await;
        git(&brain_dir, &["commit", "-m", "local commit", "--quiet"]).await;

        // Run git sync
        git_sync(&brain, &primary, &locks, &db_tx).await;

        // Verify push succeeded: clone again and check the file exists
        let verify_dir = tmp.path().join("verify");
        clone_repo(&bare, &verify_dir);
        assert!(verify_dir.join("notes/local.md").exists());
    }

    #[tokio::test]
    async fn test_git_sync_no_remote() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true);
        let primary = brain.clone();
        let locks = build_sync_locks(&[brain.clone()]);

        let (db_tx, _db_rx) = mpsc::channel::<DbRequest>(16);

        // Init without remote
        ensure_git_repo(&brain).await;

        // Should return early without error
        git_sync(&brain, &primary, &locks, &db_tx).await;
    }

    #[tokio::test]
    async fn test_conflict_resolution() {
        let tmp = TempDir::new().unwrap();
        let bare = create_bare_repo(tmp.path());

        // Create initial commit
        let init_dir = tmp.path().join("init");
        clone_repo(&bare, &init_dir);
        fs::create_dir_all(init_dir.join("notes")).unwrap();
        fs::write(
            init_dir.join("notes/shared.md"),
            "---\nname: shared\n---\n\nOriginal content\n",
        )
        .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&init_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&init_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(&init_dir)
            .output()
            .unwrap();

        // Clone two working copies
        let brain_a_dir = tmp.path().join("brain_a");
        let brain_b_dir = tmp.path().join("brain_b");
        clone_repo(&bare, &brain_a_dir);
        clone_repo(&bare, &brain_b_dir);

        // Make conflicting changes in both
        fs::write(
            brain_a_dir.join("notes/shared.md"),
            "---\nname: shared\n---\n\nChanged by A\n",
        )
        .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&brain_a_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "change A"])
            .current_dir(&brain_a_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["push"])
            .current_dir(&brain_a_dir)
            .output()
            .unwrap();

        fs::write(
            brain_b_dir.join("notes/shared.md"),
            "---\nname: shared\n---\n\nChanged by B\n",
        )
        .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(&brain_b_dir)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "change B"])
            .current_dir(&brain_b_dir)
            .output()
            .unwrap();

        // brain_b should conflict when syncing
        let brain_b = make_brain("brain-b", &brain_b_dir, true);
        let primary = Brain {
            name: "primary".to_string(),
            dir: tmp.path().join("primary"),
            primary: true,
            writable: true,
            flat: false,
            git: None,
            sync_interval: 60,
            source: None,
            refresh_interval: None,
        };
        fs::create_dir_all(&primary.dir).unwrap();

        let locks = build_sync_locks(&[brain_b.clone()]);

        let (db_tx, mut db_rx) = mpsc::channel::<DbRequest>(16);
        // Drain DB requests
        tokio::spawn(async move {
            while let Some(req) = db_rx.recv().await {
                let _ = req.reply.send(Ok("ok".to_string()));
            }
        });

        // Run sync -- should detect conflict and resolve
        git_sync(&brain_b, &primary, &locks, &db_tx).await;

        // Verify conflict file was saved
        let conflict_dir = primary.dir.join("conflicts");
        if conflict_dir.exists() {
            let entries: Vec<_> = fs::read_dir(&conflict_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(
                !entries.is_empty(),
                "Expected conflict files in conflicts/ directory"
            );

            // Check conflict file content
            let conflict_file = &entries[0].path();
            let content = fs::read_to_string(conflict_file).unwrap();
            assert!(content.contains("conflict: true"));
            assert!(content.contains("original_brain: brain-b"));
            assert!(content.contains("Changed by B"));
        }

        // Verify brain_b is no longer in rebase state
        let rebase_head = brain_b_dir.join(".git/REBASE_HEAD");
        assert!(
            !rebase_head.exists(),
            "REBASE_HEAD should not exist after conflict resolution"
        );
    }

    #[tokio::test]
    async fn test_sync_git_exclude() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true);

        // Initialize git repo
        ensure_git_repo(&brain).await;

        // Create .grugignore
        fs::write(brain_dir.join(".grugignore"), "local/\nsecret.md\n").unwrap();

        // Create a sync:false file
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        fs::write(
            brain_dir.join("notes/private.md"),
            "---\nsync: false\n---\n\nPrivate content\n",
        )
        .unwrap();

        sync_git_exclude(&brain).await;

        // Verify exclude file
        let exclude = fs::read_to_string(brain_dir.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("# managed by grug-brain"));
        assert!(exclude.contains(".grugignore"));
        assert!(exclude.contains("local/"));
        assert!(exclude.contains("secret.md"));
        assert!(exclude.contains("notes/private.md"));
    }

    #[tokio::test]
    async fn test_git_commit_file() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true);
        let locks = build_sync_locks(&[brain.clone()]);

        // Init repo and configure user
        ensure_git_repo(&brain).await;
        git(&brain_dir, &["config", "user.email", "test@test.com"]).await;
        git(&brain_dir, &["config", "user.name", "Test"]).await;

        // Create a file
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        fs::write(
            brain_dir.join("notes/test.md"),
            "---\nname: test\n---\n\nContent\n",
        )
        .unwrap();

        git_commit_file(&brain, "notes/test.md", "write", &locks).await;

        // Verify it was committed
        let log = git(&brain_dir, &["log", "--oneline"]).await;
        assert!(log.unwrap_or_default().contains("grug: write notes/test.md"));
    }

    #[tokio::test]
    async fn test_git_commit_file_local_file_skipped() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true);
        let locks = build_sync_locks(&[brain.clone()]);

        ensure_git_repo(&brain).await;
        git(&brain_dir, &["config", "user.email", "test@test.com"]).await;
        git(&brain_dir, &["config", "user.name", "Test"]).await;

        // Create a sync:false file
        fs::create_dir_all(brain_dir.join("notes")).unwrap();
        fs::write(
            brain_dir.join("notes/local.md"),
            "---\nsync: false\n---\n\nLocal only\n",
        )
        .unwrap();

        git_commit_file(&brain, "notes/local.md", "write", &locks).await;

        // Should NOT be committed
        let log = git(&brain_dir, &["log", "--oneline"]).await.unwrap_or_default();
        assert!(!log.contains("notes/local.md"));

        // But exclude file should be updated
        let exclude = fs::read_to_string(brain_dir.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("notes/local.md"));
    }

    #[tokio::test]
    async fn test_refresh_brain_writable_skips() {
        let tmp = TempDir::new().unwrap();
        let brain_dir = tmp.path().join("brain");
        fs::create_dir_all(&brain_dir).unwrap();

        let brain = make_brain("test", &brain_dir, true); // writable
        let (db_tx, _db_rx) = mpsc::channel::<DbRequest>(16);

        // Should return immediately without doing anything
        refresh_brain(&brain, &db_tx).await;
    }
}
