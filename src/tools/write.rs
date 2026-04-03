use super::GrugDb;
use crate::helpers::{slugify, today};
use crate::tools::indexing::index_file;
use std::fs;
use std::path::Path;

/// Store a memory. Saved as markdown with frontmatter, indexed for search.
/// Returns a text description of what was done.
pub fn grug_write(
    db: &mut GrugDb,
    category: &str,
    path_name: &str,
    content: &str,
    brain_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();
    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    let cat = slugify(category);
    let cat_dir = brain.dir.join(&cat);
    ensure_dir(&cat_dir);

    let slug = slugify(path_name);
    let file_path = cat_dir.join(format!("{slug}.md"));
    let exists = file_path.exists();

    let file_content = if !content.starts_with("---\n") {
        format!(
            "---\nname: {slug}\ndate: {}\ntype: memory\n---\n\n{content}\n",
            today()
        )
    } else {
        content.to_string()
    };

    fs::write(&file_path, &file_content)
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    let rel_path = file_path
        .strip_prefix(&brain.dir)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    index_file(db.conn(), &brain.name, &rel_path, &file_path, &cat)?;

    // Git commit skipped (Phase 4)

    let action = if exists { "updated" } else { "created" };
    Ok(format!("{action} {rel_path}"))
}

fn ensure_dir(path: &Path) {
    if !path.exists() {
        fs::create_dir_all(path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::test_db;

    #[test]
    fn test_grug_write_creates_file() {
        let (mut db, tmp) = test_db();
        let result = grug_write(
            &mut db,
            "notes",
            "my-test",
            "This is test content",
            None,
        )
        .unwrap();

        assert!(result.starts_with("created "));
        assert!(result.contains("notes/my-test.md"));

        // Verify file on disk
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
        grug_write(&mut db, "notes", "test", "version 1", None).unwrap();
        let result = grug_write(&mut db, "notes", "test", "version 2", None).unwrap();
        assert!(result.starts_with("updated "));
    }

    #[test]
    fn test_grug_write_preserves_frontmatter() {
        let (mut db, tmp) = test_db();
        let custom = "---\nname: custom\ndate: 2025-06-01\ntype: reference\n---\n\nCustom body";
        grug_write(&mut db, "ref", "custom", custom, None).unwrap();

        let file_path = tmp.path().join("memories/ref/custom.md");
        let content = fs::read_to_string(&file_path).unwrap();
        assert!(content.contains("date: 2025-06-01"));
        assert!(content.contains("type: reference"));
    }

    #[test]
    fn test_grug_write_readonly_brain() {
        let (mut db, _tmp) = crate::tools::test_helpers::test_db_multi();
        let result = grug_write(&mut db, "notes", "test", "content", Some("docs")).unwrap();
        assert_eq!(result, "brain \"docs\" is read-only");
    }

    #[test]
    fn test_grug_write_unknown_brain() {
        let (mut db, _tmp) = test_db();
        let result = grug_write(&mut db, "notes", "test", "content", Some("nonexistent"));
        assert!(result.is_err() || result.unwrap().contains("unknown brain"));
    }

    #[test]
    fn test_grug_write_slugifies_category() {
        let (mut db, tmp) = test_db();
        grug_write(&mut db, "My Notes!", "test", "content", None).unwrap();
        let dir = tmp.path().join("memories/my-notes");
        assert!(dir.exists());
    }
}
