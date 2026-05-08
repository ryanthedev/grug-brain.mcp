use super::GrugDb;
use crate::helpers::validate_memory_path;
use crate::tools::indexing::remove_file;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

/// Delete a memory.
///
/// By default the file is *soft-deleted*: moved to `<brain>/.trash/`
/// with a millisecond timestamp suffix. The `.trash/` directory is
/// listed in the default `.gitignore`, so soft-deleted files stay out
/// of git history. The git working tree still records the deletion of
/// the original path, so `grug-sync` will commit it.
///
/// Pass `hard=true` for the legacy hard delete (`fs::remove_file`).
pub fn grug_delete(
    db: &mut GrugDb,
    category: &str,
    path_name: &str,
    brain_name: Option<&str>,
    hard: bool,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    validate_memory_path(category)?;
    validate_memory_path(path_name)?;

    let file = if path_name.contains('/') {
        path_name.split('/').next_back().unwrap_or(path_name)
    } else {
        path_name
    };
    let t = if file.ends_with(".md") {
        file.to_string()
    } else {
        format!("{file}.md")
    };

    let file_path = brain.dir.join(category).join(&t);
    if !file_path.exists() {
        return Ok(format!("not found: {category}/{file}"));
    }

    let rel_path = format!("{category}/{t}");

    let lock = db.path_locks().for_path(&brain.name, &rel_path);
    let _guard = lock.lock().expect("path lock poisoned");

    if hard {
        fs::remove_file(&file_path)
            .map_err(|e| format!("failed to delete {}: {e}", file_path.display()))?;
    } else {
        let trash_dir = brain.dir.join(".trash");
        fs::create_dir_all(&trash_dir)
            .map_err(|e| format!("failed to create trash dir: {e}"))?;

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        // Flatten directory separators so we never need to mkdir-p inside .trash
        // and so trashed files have unambiguous, single-level names.
        let stem = rel_path.trim_end_matches(".md").replace('/', "--");
        let trash_name = format!("{stem}-{ts}.md");
        let trash_path = trash_dir.join(&trash_name);

        fs::rename(&file_path, &trash_path).map_err(|e| {
            format!(
                "failed to move {} to trash {}: {e}",
                file_path.display(),
                trash_path.display()
            )
        })?;
    }

    remove_file(db.conn(), &brain.name, &rel_path)?;

    db.enqueue_git_commit(&brain.name, &rel_path, "delete");

    Ok(format!("deleted {rel_path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi, test_db_with_git};

    #[test]
    fn test_delete_basic() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/doomed.md",
            "---\nname: doomed\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/doomed.md", &f, "notes").unwrap();

        let result = grug_delete(&mut db, "notes", "doomed", None, false).unwrap();
        assert_eq!(result, "deleted notes/doomed.md");
        assert!(!f.exists());

        // DB cleanup
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_delete_not_found() {
        let (mut db, _tmp) = test_db();
        let result = grug_delete(&mut db, "notes", "nonexistent", None, false).unwrap();
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_delete_readonly() {
        let (mut db, _tmp) = test_db_multi();
        let result = grug_delete(&mut db, "cat", "test", Some("docs"), false).unwrap();
        assert_eq!(result, "brain \"docs\" is read-only");
    }

    #[test]
    fn test_delete_adds_md_extension() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/test.md", "---\nname: test\n---\n\nBody");
        let f = brain_dir.join("notes/test.md");
        index_file(db.conn(), "memories", "notes/test.md", &f, "notes").unwrap();

        let result = grug_delete(&mut db, "notes", "test", None, false).unwrap();
        assert_eq!(result, "deleted notes/test.md");
        assert!(!f.exists());
    }

    #[test]
    fn test_delete_with_path_slash() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/hello.md", "---\nname: hello\n---\n\nBody");
        let f = brain_dir.join("notes/hello.md");
        index_file(db.conn(), "memories", "notes/hello.md", &f, "notes").unwrap();

        let result = grug_delete(&mut db, "notes", "sub/hello", None, false).unwrap();
        assert_eq!(result, "deleted notes/hello.md");
    }

    // -- DW-1.3: soft delete to .trash/ + git commit emission --

    #[test]
    fn test_dw_1_3_soft_delete_moves_to_trash() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/keep-me.md",
            "---\nname: keep-me\n---\n\nValuable content",
        );
        index_file(db.conn(), "memories", "notes/keep-me.md", &f, "notes").unwrap();

        grug_delete(&mut db, "notes", "keep-me", None, false).unwrap();

        // Original gone
        assert!(!f.exists(), "original file should be gone");

        // .trash/ has a file matching the flattened name pattern
        let trash_dir = brain_dir.join(".trash");
        assert!(trash_dir.exists(), ".trash should exist");
        let entries: Vec<_> = fs::read_dir(&trash_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries.len(), 1, "expected exactly one trash file: {entries:?}");
        let name = &entries[0];
        assert!(name.starts_with("notes--keep-me-"), "got {name}");
        assert!(name.ends_with(".md"), "got {name}");

        // Content preserved
        let content = fs::read_to_string(trash_dir.join(name)).unwrap();
        assert!(content.contains("Valuable content"));
    }

    #[test]
    fn test_dw_1_3_soft_delete_emits_git_commit_request() {
        let (mut db, tmp, mut rx) = test_db_with_git();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(&brain_dir, "notes/x.md", "---\nname: x\n---\n\nBody");
        index_file(db.conn(), "memories", "notes/x.md", &f, "notes").unwrap();

        grug_delete(&mut db, "notes", "x", None, false).unwrap();

        let req = rx.try_recv().expect("expected GitCommitRequest");
        assert_eq!(req.brain, "memories");
        assert_eq!(req.rel_path, "notes/x.md");
        assert_eq!(req.action, "delete");
    }

    #[test]
    fn test_dw_1_3_hard_delete_removes_file() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(&brain_dir, "notes/zap.md", "---\nname: zap\n---\n\nBye");
        index_file(db.conn(), "memories", "notes/zap.md", &f, "notes").unwrap();

        grug_delete(&mut db, "notes", "zap", None, true).unwrap();

        assert!(!f.exists());
        let trash_dir = brain_dir.join(".trash");
        assert!(
            !trash_dir.exists() || fs::read_dir(&trash_dir).unwrap().next().is_none(),
            "hard delete should not populate .trash"
        );
    }

    // -- DW-1.4: path validation in delete --

    #[test]
    fn test_dw_1_4_delete_rejects_traversal() {
        let (mut db, _tmp) = test_db();
        let r = grug_delete(&mut db, "..", "x", None, false);
        assert!(r.is_err());
        let r = grug_delete(&mut db, "notes", "../escape", None, false);
        assert!(r.is_err());
    }

    #[test]
    fn test_dw_1_4_delete_rejects_null_byte() {
        let (mut db, _tmp) = test_db();
        let r = grug_delete(&mut db, "notes\0", "x", None, false);
        assert!(r.is_err());
    }
}
