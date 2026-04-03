use super::GrugDb;
use crate::tools::indexing::remove_file;
use std::fs;

/// Delete a memory from disk and database.
pub fn grug_delete(
    db: &mut GrugDb,
    category: &str,
    path_name: &str,
    brain_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    let file = if path_name.contains('/') {
        path_name.split('/').last().unwrap_or(path_name)
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

    fs::remove_file(&file_path)
        .map_err(|e| format!("failed to delete {}: {e}", file_path.display()))?;

    let rel_path = format!("{category}/{t}");
    remove_file(db.conn(), &brain.name, &rel_path)?;

    // Git commit skipped (Phase 4)

    Ok(format!("deleted {rel_path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi};

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

        let result = grug_delete(&mut db, "notes", "doomed", None).unwrap();
        assert_eq!(result, "deleted notes/doomed.md");
        assert!(!f.exists());

        // Verify DB cleanup
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
        let result = grug_delete(&mut db, "notes", "nonexistent", None).unwrap();
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_delete_readonly() {
        let (mut db, _tmp) = test_db_multi();
        let result = grug_delete(&mut db, "cat", "test", Some("docs")).unwrap();
        assert_eq!(result, "brain \"docs\" is read-only");
    }

    #[test]
    fn test_delete_adds_md_extension() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(
            &brain_dir,
            "notes/test.md",
            "---\nname: test\n---\n\nBody",
        );
        let f = brain_dir.join("notes/test.md");
        index_file(db.conn(), "memories", "notes/test.md", &f, "notes").unwrap();

        // Delete without .md extension
        let result = grug_delete(&mut db, "notes", "test", None).unwrap();
        assert_eq!(result, "deleted notes/test.md");
        assert!(!f.exists());
    }

    #[test]
    fn test_delete_with_path_slash() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(
            &brain_dir,
            "notes/hello.md",
            "---\nname: hello\n---\n\nBody",
        );
        let f = brain_dir.join("notes/hello.md");
        index_file(db.conn(), "memories", "notes/hello.md", &f, "notes").unwrap();

        // path_name contains a slash -- JS extracts last segment
        let result = grug_delete(&mut db, "notes", "sub/hello", None).unwrap();
        assert_eq!(result, "deleted notes/hello.md");
    }
}
