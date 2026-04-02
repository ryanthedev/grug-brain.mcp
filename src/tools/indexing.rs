use crate::parsing::{extract_body, extract_description, extract_frontmatter};
use crate::types::Brain;
use crate::walker::{get_categories, walk_files};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Index a single file into the database.
/// Reads content from disk, extracts metadata, and upserts FTS + files rows.
pub fn index_file(
    conn: &Connection,
    brain_name: &str,
    rel_path: &str,
    full_path: &Path,
    category: &str,
) -> Result<(), String> {
    let content = fs::read_to_string(full_path)
        .map_err(|e| format!("failed to read {}: {e}", full_path.display()))?;

    let fm = extract_frontmatter(&content);
    let body = extract_body(&content);
    let desc = extract_description(&content);

    // Name: frontmatter name > title > file stem
    let name = fm
        .get("name")
        .or_else(|| fm.get("title"))
        .cloned()
        .unwrap_or_else(|| {
            Path::new(rel_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(rel_path)
                .to_string()
        });

    let date = fm.get("date").cloned().unwrap_or_default();

    // Delete existing FTS entry, then insert fresh
    conn.execute(
        "DELETE FROM brain_fts WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete fts: {e}"))?;

    conn.execute(
        "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![rel_path, brain_name, category, name, date, desc, body],
    )
    .map_err(|e| format!("insert fts: {e}"))?;

    // Get file mtime and upsert files row
    let mtime = fs::metadata(full_path)
        .map(|m| {
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64() * 1000.0) // Store as milliseconds like JS
                .unwrap_or(0.0)
        })
        .unwrap_or(0.0);

    conn.execute(
        "INSERT OR REPLACE INTO files (brain, path, mtime) VALUES (?1, ?2, ?3)",
        rusqlite::params![brain_name, rel_path, mtime],
    )
    .map_err(|e| format!("upsert file: {e}"))?;

    Ok(())
}

/// Remove a file from all database tables (FTS, files, dream_log, cross_links).
pub fn remove_file(conn: &Connection, brain_name: &str, rel_path: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM brain_fts WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete fts: {e}"))?;

    conn.execute(
        "DELETE FROM files WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete file: {e}"))?;

    conn.execute(
        "DELETE FROM dream_log WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete dream_log: {e}"))?;

    conn.execute(
        "DELETE FROM cross_links WHERE (brain_a = ?1 AND path_a = ?2) OR (brain_b = ?1 AND path_b = ?2)",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete cross_links: {e}"))?;

    Ok(())
}

/// Full sync: walk disk, diff against indexed, index new/changed, remove stale.
/// Returns (on_disk_count, indexed_count, removed_count).
pub fn sync_brain(conn: &Connection, brain: &Brain) -> Result<(usize, usize, usize), String> {
    // Get all currently indexed files + mtime for this brain
    let mut stmt = conn
        .prepare("SELECT path, mtime FROM files WHERE brain = ?1")
        .map_err(|e| format!("prepare: {e}"))?;
    let indexed: HashMap<String, f64> = stmt
        .query_map([&brain.name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|e| format!("query indexed: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    let mut on_disk = HashSet::new();
    let mut indexed_count = 0;

    if brain.flat {
        // Flat brain: all files are in brain.dir, category = brain name
        for full_path in walk_files(&brain.dir) {
            let rel_path = full_path
                .strip_prefix(&brain.dir)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            if rel_path.is_empty() {
                continue;
            }
            on_disk.insert(rel_path.clone());

            let mtime = file_mtime_ms(&full_path);
            if indexed.get(&rel_path).copied() != Some(mtime) {
                if index_file(conn, &brain.name, &rel_path, &full_path, &brain.name).is_ok() {
                    indexed_count += 1;
                }
            }
        }
    } else {
        // Category brain: walk each category subdirectory
        for cat in get_categories(&brain.dir) {
            let cat_dir = brain.dir.join(&cat);
            for full_path in walk_files(&cat_dir) {
                let rel_path = full_path
                    .strip_prefix(&brain.dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                if rel_path.is_empty() {
                    continue;
                }
                on_disk.insert(rel_path.clone());

                let mtime = file_mtime_ms(&full_path);
                if indexed.get(&rel_path).copied() != Some(mtime) {
                    if index_file(conn, &brain.name, &rel_path, &full_path, &cat).is_ok() {
                        indexed_count += 1;
                    }
                }
            }
        }
    }

    // Remove files that are indexed but no longer on disk
    let mut removed_count = 0;
    for path in indexed.keys() {
        if !on_disk.contains(path) {
            if remove_file(conn, &brain.name, path).is_ok() {
                removed_count += 1;
            }
        }
    }

    Ok((on_disk.len(), indexed_count, removed_count))
}

/// Get file mtime in milliseconds (matching JS mtimeMs).
fn file_mtime_ms(path: &Path) -> f64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::{create_brain_file, test_db};

    #[test]
    fn test_index_file_basic() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let full = create_brain_file(
            &brain_dir,
            "notes/test.md",
            "---\nname: test-note\ndate: 2025-03-15\n---\n\nThis is the body.",
        );

        index_file(db.conn(), "memories", "notes/test.md", &full, "notes").unwrap();

        // Verify FTS row
        let name: String = db
            .conn()
            .query_row(
                "SELECT name FROM brain_fts WHERE brain = 'memories' AND path = 'notes/test.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "test-note");

        // Verify files row
        let mtime: f64 = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = 'memories' AND path = 'notes/test.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(mtime > 0.0);
    }

    #[test]
    fn test_index_file_no_frontmatter_name() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let full = create_brain_file(&brain_dir, "cat/my-file.md", "Just body, no frontmatter");

        index_file(db.conn(), "memories", "cat/my-file.md", &full, "cat").unwrap();

        // Name should be derived from filename stem
        let name: String = db
            .conn()
            .query_row(
                "SELECT name FROM brain_fts WHERE path = 'cat/my-file.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "my-file");
    }

    #[test]
    fn test_remove_file() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let full = create_brain_file(
            &brain_dir,
            "cat/doomed.md",
            "---\nname: doomed\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "cat/doomed.md", &full, "cat").unwrap();

        // Add a dream_log entry
        db.conn()
            .execute(
                "INSERT INTO dream_log (brain, path, reviewed_at, mtime_at_review) VALUES ('memories', 'cat/doomed.md', '2025-01-01', 1.0)",
                [],
            )
            .unwrap();

        // Add a cross_link entry
        db.conn()
            .execute(
                "INSERT INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES ('memories', 'cat/doomed.md', 'memories', 'other/file.md', 0.5, '2025-01-01')",
                [],
            )
            .unwrap();

        remove_file(db.conn(), "memories", "cat/doomed.md").unwrap();

        // Verify all rows are gone
        let fts_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE path = 'cat/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 0);

        let file_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE path = 'cat/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(file_count, 0);

        let dream_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM dream_log WHERE path = 'cat/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dream_count, 0);

        let link_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM cross_links WHERE path_a = 'cat/doomed.md' OR path_b = 'cat/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(link_count, 0);
    }

    #[test]
    fn test_sync_brain_basic() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\n---\n\nContent A",
        );
        create_brain_file(
            &brain_dir,
            "notes/b.md",
            "---\nname: b\n---\n\nContent B",
        );

        let brain = db.config().primary_brain().clone();
        let (on_disk, indexed, removed) = sync_brain(db.conn(), &brain).unwrap();
        assert_eq!(on_disk, 2);
        assert_eq!(indexed, 2);
        assert_eq!(removed, 0);

        // Second sync with no changes should index 0
        let (on_disk2, indexed2, removed2) = sync_brain(db.conn(), &brain).unwrap();
        assert_eq!(on_disk2, 2);
        assert_eq!(indexed2, 0);
        assert_eq!(removed2, 0);
    }

    #[test]
    fn test_sync_brain_removes_deleted() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let file_path = create_brain_file(
            &brain_dir,
            "notes/temp.md",
            "---\nname: temp\n---\n\nTemp content",
        );

        let brain = db.config().primary_brain().clone();
        sync_brain(db.conn(), &brain).unwrap();

        // Delete the file from disk
        std::fs::remove_file(file_path).unwrap();

        let (on_disk, indexed, removed) = sync_brain(db.conn(), &brain).unwrap();
        assert_eq!(on_disk, 0);
        assert_eq!(indexed, 0);
        assert_eq!(removed, 1);
    }
}
