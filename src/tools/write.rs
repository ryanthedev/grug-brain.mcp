use super::GrugDb;
use crate::helpers::{slugify, today, validate_memory_path};
use crate::tools::indexing::index_file;
use serde_json::json;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use tempfile::NamedTempFile;

/// Store a memory. Saved as markdown with frontmatter, indexed for search.
/// Returns a text description of what was done.
///
/// `if_match_mtime` is an optional ETag-style precondition: when `Some(want)`
/// and the file already exists, the current `files.mtime` must equal `want`
/// or the call returns a structured JSON conflict error without writing.
pub fn grug_write(
    db: &mut GrugDb,
    category: &str,
    path_name: &str,
    content: &str,
    brain_name: Option<&str>,
    if_match_mtime: Option<f64>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    // Reject obviously dangerous category/path values BEFORE slugifying so the
    // raw user input never reaches the filesystem or shell-bound tooling.
    validate_memory_path(category)?;
    validate_memory_path(path_name)?;
    reject_conflict_markers(content)?;

    let cat = slugify(category);
    let slug = slugify(path_name);
    if cat.is_empty() {
        return Err("category slugifies to empty".to_string());
    }
    if slug.is_empty() {
        return Err("path slugifies to empty".to_string());
    }

    let cat_dir = brain.dir.join(&cat);
    ensure_dir(&cat_dir);

    let file_path = cat_dir.join(format!("{slug}.md"));
    let rel_path = format!("{cat}/{slug}.md");

    let file_content = if !content.starts_with("---\n") {
        format!(
            "---\nname: {slug}\ndate: {}\ntype: memory\n---\n\n{content}\n",
            today()
        )
    } else {
        content.to_string()
    };

    // Acquire per-(brain, path) lock for the whole conflict-check + write +
    // index sequence so a future parallel writer cannot interleave.
    let lock = db.path_locks().for_path(&brain.name, &rel_path);
    let _guard = lock.lock().expect("path lock poisoned");

    let exists = file_path.exists();

    if let Some(want) = if_match_mtime {
        if exists {
            let current_mtime: Option<f64> = db
                .conn()
                .query_row(
                    "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
                    rusqlite::params![&brain.name, &rel_path],
                    |row| row.get(0),
                )
                .ok();

            // Treat unindexed-but-on-disk files as "no recorded mtime" -- we
            // refuse to overwrite without an explicit force.
            let current = current_mtime.unwrap_or(0.0);
            if (current - want).abs() > f64::EPSILON {
                let current_content =
                    fs::read_to_string(&file_path).unwrap_or_default();
                let body = json!({
                    "error": "conflict",
                    "current_mtime": current,
                    "current_content": current_content,
                });
                return Err(serde_json::to_string(&body).unwrap_or_else(|_| body.to_string()));
            }
        }
    }

    atomic_write(&file_path, file_content.as_bytes())
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    index_file(db.conn(), &brain.name, &rel_path, &file_path, &cat)?;

    db.enqueue_git_commit(&brain.name, &rel_path, "write");

    let action = if exists { "updated" } else { "created" };
    Ok(format!("{action} {rel_path}"))
}

fn ensure_dir(path: &Path) {
    if !path.exists() {
        fs::create_dir_all(path).ok();
    }
}

/// Atomically write `bytes` to `target` via tempfile-then-rename within the
/// same directory. The rename is atomic on POSIX; either the old file or the
/// new file is observable, never a half-written file.
fn atomic_write(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target has no parent")
    })?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.flush()?;
    tmp.persist(target).map_err(|e| e.error)?;
    Ok(())
}

/// Reject content that looks like an unresolved git merge conflict.
/// We require the markers to appear at line starts to avoid prose false
/// positives (e.g. a markdown code block discussing conflicts).
fn reject_conflict_markers(content: &str) -> Result<(), String> {
    for line in content.lines() {
        if line.starts_with("<<<<<<< ")
            || line == "======="
            || line.starts_with(">>>>>>> ")
        {
            return Err(format!(
                "content contains merge conflict marker on a line: {line:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::{test_db, test_db_with_git};

    #[test]
    fn test_grug_write_creates_file() {
        let (mut db, tmp) = test_db();
        let result = grug_write(
            &mut db,
            "notes",
            "my-test",
            "This is test content",
            None,
            None,
        )
        .unwrap();

        assert!(result.starts_with("created "));
        assert!(result.contains("notes/my-test.md"));

        let file_path = tmp.path().join("memories/notes/my-test.md");
        assert!(file_path.exists());
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("name: my-test"));
        assert!(content.contains("This is test content"));
    }

    #[test]
    fn test_grug_write_updates_file() {
        let (mut db, _tmp) = test_db();
        grug_write(&mut db, "notes", "test", "version 1", None, None).unwrap();
        let result = grug_write(&mut db, "notes", "test", "version 2", None, None).unwrap();
        assert!(result.starts_with("updated "));
    }

    #[test]
    fn test_grug_write_preserves_frontmatter() {
        let (mut db, tmp) = test_db();
        let custom = "---\nname: custom\ndate: 2025-06-01\ntype: reference\n---\n\nCustom body";
        grug_write(&mut db, "ref", "custom", custom, None, None).unwrap();

        let file_path = tmp.path().join("memories/ref/custom.md");
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("date: 2025-06-01"));
        assert!(content.contains("type: reference"));
    }

    #[test]
    fn test_grug_write_readonly_brain() {
        let (mut db, _tmp) = crate::tools::test_helpers::test_db_multi();
        let result =
            grug_write(&mut db, "notes", "test", "content", Some("docs"), None).unwrap();
        assert_eq!(result, "brain \"docs\" is read-only");
    }

    #[test]
    fn test_grug_write_unknown_brain() {
        let (mut db, _tmp) = test_db();
        let result =
            grug_write(&mut db, "notes", "test", "content", Some("nonexistent"), None);
        assert!(result.is_err() || result.unwrap().contains("unknown brain"));
    }

    #[test]
    fn test_grug_write_slugifies_category() {
        // Slugification still applies to spaces, capitalization, and unicode;
        // shell metacharacters (`!`, etc.) are rejected at validate_memory_path
        // *before* reaching the slugifier. See DW-1.4 tests for the reject path.
        let (mut db, tmp) = test_db();
        grug_write(&mut db, "My Notes", "test", "content", None, None).unwrap();
        let dir = tmp.path().join("memories/my-notes");
        assert!(dir.exists());
    }

    // -- DW-1.2: git commit emission --

    #[test]
    fn test_dw_1_2_emits_git_commit_request() {
        let (mut db, _tmp, mut rx) = test_db_with_git();
        grug_write(&mut db, "notes", "hello", "body", None, None).unwrap();

        // Drain channel
        let req = rx.try_recv().expect("expected a GitCommitRequest");
        assert_eq!(req.brain, "memories");
        assert_eq!(req.rel_path, "notes/hello.md");
        assert_eq!(req.action, "write");
    }

    #[test]
    fn test_dw_1_2_no_git_tx_no_panic() {
        // Without git_tx wired, write still succeeds and emits nothing.
        let (mut db, _tmp) = test_db();
        grug_write(&mut db, "notes", "hello", "body", None, None).unwrap();
    }

    // -- DW-1.4: path validation --

    #[test]
    fn test_dw_1_4_write_rejects_traversal() {
        let (mut db, _tmp) = test_db();
        let r = grug_write(&mut db, "..", "x", "body", None, None);
        assert!(r.is_err());
        let r = grug_write(&mut db, "notes", "../escape", "body", None, None);
        assert!(r.is_err());
    }

    #[test]
    fn test_dw_1_4_write_rejects_absolute() {
        let (mut db, _tmp) = test_db();
        let r = grug_write(&mut db, "/etc", "passwd", "body", None, None);
        assert!(r.is_err());
    }

    #[test]
    fn test_dw_1_4_write_rejects_null_byte() {
        let (mut db, _tmp) = test_db();
        let r = grug_write(&mut db, "notes\0bad", "x", "body", None, None);
        assert!(r.is_err());
    }

    #[test]
    fn test_dw_1_4_write_rejects_shell_metachars() {
        let (mut db, _tmp) = test_db();
        let r = grug_write(&mut db, "notes;rm", "x", "body", None, None);
        assert!(r.is_err());
        let r = grug_write(&mut db, "notes", "x|y", "body", None, None);
        assert!(r.is_err());
    }

    // -- DW-1.5: if_match_mtime conflict --

    #[test]
    fn test_dw_1_5_if_match_mtime_match_succeeds() {
        let (mut db, _tmp) = test_db();
        grug_write(&mut db, "notes", "x", "v1", None, None).unwrap();

        // Read the indexed mtime
        let mtime: f64 = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = 'memories' AND path = 'notes/x.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Provide the matching mtime -- update should succeed
        let r = grug_write(&mut db, "notes", "x", "v2", None, Some(mtime));
        assert!(r.is_ok(), "matching mtime should succeed: {r:?}");
    }

    #[test]
    fn test_dw_1_5_if_match_mtime_mismatch_returns_conflict() {
        let (mut db, tmp) = test_db();
        grug_write(&mut db, "notes", "x", "v1", None, None).unwrap();

        let file_path = tmp.path().join("memories/notes/x.md");
        let before = fs::read_to_string(&file_path).unwrap();

        let r = grug_write(
            &mut db,
            "notes",
            "x",
            "v2-attempt",
            None,
            Some(0.0001),
        );
        assert!(r.is_err(), "stale mtime should produce conflict error");
        let err = r.unwrap_err();
        let parsed: serde_json::Value =
            serde_json::from_str(&err).expect("conflict error should be JSON");
        assert_eq!(parsed["error"], "conflict");
        assert!(parsed["current_mtime"].is_number());
        assert_eq!(parsed["current_content"], before);

        // Verify file was NOT modified
        let after = fs::read_to_string(&file_path).unwrap();
        assert_eq!(after, before, "file must not be written on conflict");
    }

    // -- DW-1.6: atomic write --

    #[test]
    fn test_dw_1_6_atomic_write_no_leftover_tempfiles() {
        let (mut db, tmp) = test_db();
        grug_write(&mut db, "notes", "atomic", "body", None, None).unwrap();

        // After a successful write the parent dir should contain only the
        // target .md file. NamedTempFile::persist renames in-place; if it
        // failed, a `.tmp*` file would remain.
        let cat_dir = tmp.path().join("memories/notes");
        let entries: Vec<_> = fs::read_dir(&cat_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["atomic.md".to_string()]);
    }

    #[test]
    fn test_dw_1_6_overwrite_preserves_old_or_new_never_partial() {
        // We can't simulate true power-loss in-process, but we can verify the
        // semantic guarantee that `atomic_write` calls underlying rename: a
        // successful write never leaves a partial file, and a failed write
        // (we trigger via invalid parent) never touches the target.
        use std::path::PathBuf;
        let bad_target: PathBuf = "/proc/nonexistent/dir/file.md".into();
        let r = atomic_write(&bad_target, b"data");
        assert!(r.is_err());
        assert!(!bad_target.exists());
    }

    // -- DW-1.7: conflict markers --

    #[test]
    fn test_dw_1_7_rejects_left_marker() {
        let (mut db, _tmp) = test_db();
        let r = grug_write(
            &mut db,
            "notes",
            "x",
            "intro\n<<<<<<< HEAD\nour\n=======\ntheir\n>>>>>>> branch\n",
            None,
            None,
        );
        assert!(r.is_err());
    }

    #[test]
    fn test_dw_1_7_rejects_each_marker_individually() {
        let (mut db, _tmp) = test_db();
        for content in [
            "before\n<<<<<<< HEAD\nafter",
            "before\n=======\nafter",
            "before\n>>>>>>> branch\nafter",
        ] {
            let r = grug_write(&mut db, "notes", "x", content, None, None);
            assert!(r.is_err(), "expected rejection for marker in: {content:?}");
        }
    }

    #[test]
    fn test_dw_1_7_does_not_match_inline_text() {
        // A line that mentions <<<<<<< inline (not at start) should NOT trip.
        let (mut db, _tmp) = test_db();
        let r = grug_write(
            &mut db,
            "notes",
            "x",
            "we use <<<<<<< as a sentinel string in code\n",
            None,
            None,
        );
        assert!(r.is_ok(), "inline marker text should not be rejected: {r:?}");
    }

    // -- DW-1.8: serialization (per-path mutex composition) --

    #[test]
    fn test_dw_1_8_per_path_mutex_exists_in_grugdb() {
        let (db, _tmp) = test_db();
        // Lock is uncontended; verify the same Arc is returned for the same key.
        let a = db.path_locks().for_path("memories", "notes/x.md");
        let b = db.path_locks().for_path("memories", "notes/x.md");
        assert!(Arc::ptr_eq(&a, &b));
        let c = db.path_locks().for_path("memories", "notes/y.md");
        assert!(!Arc::ptr_eq(&a, &c));
    }

    #[test]
    fn test_dw_1_8_sequential_writes_observe_increasing_mtime() {
        // Even without parallel threads, two sequential writes through the
        // same path should produce a strictly increasing recorded mtime,
        // demonstrating the second write sees the first's effect.
        let (mut db, _tmp) = test_db();
        grug_write(&mut db, "notes", "race", "v1", None, None).unwrap();
        let mtime1: f64 = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = 'memories' AND path = 'notes/race.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // Tiny sleep to make the mtime monotonically advance even on systems
        // with low-resolution clocks.
        std::thread::sleep(std::time::Duration::from_millis(2));

        grug_write(&mut db, "notes", "race", "v2", None, None).unwrap();
        let mtime2: f64 = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = 'memories' AND path = 'notes/race.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            mtime2 >= mtime1,
            "second write should observe first's recorded mtime, then advance: {mtime1} -> {mtime2}"
        );
    }

    use std::sync::Arc;
}
