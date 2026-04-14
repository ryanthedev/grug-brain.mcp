use super::GrugDb;
use crate::tools::indexing::index_file;
use std::fs;

pub use crate::client::EditEntry;

/// Edit a memory in place by applying substring find-and-replace edits.
/// All edits are validated before writing — if any old string is not found,
/// no changes are made to disk.
pub fn grug_update(
    db: &mut GrugDb,
    category: &str,
    path_name: &str,
    edits: &[EditEntry],
    brain_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let brain = db.resolve_brain(brain_name)?.clone();

    if !brain.writable {
        return Ok(format!("brain \"{}\" is read-only", brain.name));
    }

    // Path resolution — delete.rs style: strip leading segments, append .md if missing
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

    let mut content = fs::read_to_string(&file_path)
        .map_err(|e| format!("failed to read {}: {e}", file_path.display()))?;

    // Apply edits sequentially to in-memory content.
    // If any old string is not found, return Err before writing to disk.
    for (i, edit) in edits.iter().enumerate() {
        if let Some(pos) = content.find(&edit.old) {
            content = format!(
                "{}{}{}",
                &content[..pos],
                &edit.new,
                &content[pos + edit.old.len()..]
            );
        } else {
            return Err(format!(
                "edit {}: old string not found: {:?}",
                i + 1,
                truncate(&edit.old, 80)
            ));
        }
    }

    fs::write(&file_path, &content)
        .map_err(|e| format!("failed to write {}: {e}", file_path.display()))?;

    let rel_path = format!("{category}/{t}");
    index_file(db.conn(), &brain.name, &rel_path, &file_path, category)?;

    let edit_word = if edits.len() == 1 { "edit" } else { "edits" };
    Ok(format!("updated {rel_path} ({} {edit_word})", edits.len()))
}

/// Truncate a string for error display.
fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Find a char boundary at or before max
        let end = s.floor_char_boundary(max);
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi};

    #[test]
    fn test_basic_single_edit() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/greet.md", "hello world");

        let edits = vec![EditEntry {
            old: "hello".to_string(),
            new: "goodbye".to_string(),
        }];
        let result = grug_update(&mut db, "notes", "greet", &edits, None).unwrap();
        assert_eq!(result, "updated notes/greet.md (1 edit)");

        let content = std::fs::read_to_string(brain_dir.join("notes/greet.md")).unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[test]
    fn test_batch_edits() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/multi.md", "aaa bbb ccc");

        let edits = vec![
            EditEntry {
                old: "aaa".to_string(),
                new: "xxx".to_string(),
            },
            EditEntry {
                old: "bbb".to_string(),
                new: "yyy".to_string(),
            },
        ];
        let result = grug_update(&mut db, "notes", "multi", &edits, None).unwrap();
        assert_eq!(result, "updated notes/multi.md (2 edits)");

        let content = std::fs::read_to_string(brain_dir.join("notes/multi.md")).unwrap();
        assert_eq!(content, "xxx yyy ccc");
    }

    #[test]
    fn test_file_not_found() {
        let (mut db, _tmp) = test_db();
        let edits = vec![EditEntry {
            old: "x".to_string(),
            new: "y".to_string(),
        }];
        let result = grug_update(&mut db, "notes", "nonexistent", &edits, None).unwrap();
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_old_string_not_found() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/oops.md", "hello");

        let edits = vec![EditEntry {
            old: "NONEXISTENT".to_string(),
            new: "x".to_string(),
        }];
        let result = grug_update(&mut db, "notes", "oops", &edits, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("old string not found"));
    }

    #[test]
    fn test_no_partial_write_on_failure() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/safe.md", "aaa bbb");

        let edits = vec![
            EditEntry {
                old: "aaa".to_string(),
                new: "xxx".to_string(),
            },
            EditEntry {
                old: "MISSING".to_string(),
                new: "yyy".to_string(),
            },
        ];
        let result = grug_update(&mut db, "notes", "safe", &edits, None);
        assert!(result.is_err());

        // File on disk must be unchanged
        let content = std::fs::read_to_string(brain_dir.join("notes/safe.md")).unwrap();
        assert_eq!(content, "aaa bbb");
    }

    #[test]
    fn test_readonly_brain() {
        let (mut db, _tmp) = test_db_multi();
        let edits = vec![EditEntry {
            old: "x".to_string(),
            new: "y".to_string(),
        }];
        let result = grug_update(&mut db, "cat", "test", &edits, Some("docs")).unwrap();
        assert_eq!(result, "brain \"docs\" is read-only");
    }

    #[test]
    fn test_md_extension_handling() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/ext.md", "before");

        let edits = vec![EditEntry {
            old: "before".to_string(),
            new: "after".to_string(),
        }];
        // Pass path without .md extension
        let result = grug_update(&mut db, "notes", "ext", &edits, None).unwrap();
        assert!(result.contains("updated"));

        let content = std::fs::read_to_string(brain_dir.join("notes/ext.md")).unwrap();
        assert_eq!(content, "after");
    }

    #[test]
    fn test_reindex_after_edit() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/idx.md",
            "---\nname: idx\n---\n\nold body text",
        );
        index_file(db.conn(), "memories", "notes/idx.md", &f, "notes").unwrap();

        let edits = vec![EditEntry {
            old: "old body".to_string(),
            new: "new body".to_string(),
        }];
        grug_update(&mut db, "notes", "idx", &edits, None).unwrap();

        // Verify FTS was updated
        let body: String = db
            .conn()
            .query_row(
                "SELECT body FROM brain_fts WHERE brain = 'memories' AND path = 'notes/idx.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(body.contains("new body"));
        assert!(!body.contains("old body"));
    }
}
