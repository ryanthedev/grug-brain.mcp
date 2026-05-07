use crate::helpers::today;
use crate::parsing::{extract_body, extract_description, extract_frontmatter, parse_links, parse_tags};
use crate::tools::tfidf;
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

    // Re-populate links + tags. We delete first so a re-index of an edited
    // file replaces (rather than accumulates) its outgoing references and
    // tags. Target-side `links` rows pointing at this file are intentionally
    // left untouched -- those represent OTHER memories' outgoing references.
    conn.execute(
        "DELETE FROM links WHERE brain = ?1 AND src_path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete src links: {e}"))?;
    conn.execute(
        "DELETE FROM tags WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete tags: {e}"))?;

    let now = today();
    for target in parse_links(&content) {
        let (target_brain, target_path, unresolved) =
            resolve_link(conn, brain_name, &target);
        // Insert OR IGNORE: PK is composite (brain, src, target_brain,
        // target_path, target_name_unresolved); duplicate parses dedupe at
        // SQL boundary too.
        conn.execute(
            "INSERT OR IGNORE INTO links (brain, src_path, target_brain, target_path, target_name_unresolved, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                brain_name,
                rel_path,
                target_brain,
                target_path,
                unresolved,
                now
            ],
        )
        .map_err(|e| format!("insert link: {e}"))?;
    }
    for tag in parse_tags(&content) {
        conn.execute(
            "INSERT OR IGNORE INTO tags (brain, path, tag) VALUES (?1, ?2, ?3)",
            rusqlite::params![brain_name, rel_path, tag],
        )
        .map_err(|e| format!("insert tag: {e}"))?;
    }

    // Compute and store TF-IDF term weights for this document
    tfidf::compute_and_store_weights(conn, brain_name, rel_path)?;

    Ok(())
}

/// Resolve a `[[target]]` string to a stored row triple
/// `(target_brain, target_path, target_name_unresolved)`. Within-brain only:
/// cross-brain links are deferred to a later plan.
///
/// - If the target contains `/`, it's treated as `category/slug` and we look
///   up `path = "<category>/<slug>.md"` in the same brain.
/// - Else we look up by `name` column in `brain_fts` within the same brain.
/// - On miss, store `target_name_unresolved` only; both target columns NULL
///   so future re-resolution can fill them in.
fn resolve_link(
    conn: &Connection,
    brain_name: &str,
    target: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return (None, None, Some(target.to_string()));
    }

    if trimmed.contains('/') {
        // Treat as category/slug. Try exact path match first.
        let candidate_path = if trimmed.ends_with(".md") {
            trimmed.to_string()
        } else {
            format!("{trimmed}.md")
        };
        let found: Option<String> = conn
            .query_row(
                "SELECT path FROM brain_fts WHERE brain = ?1 AND path = ?2 LIMIT 1",
                rusqlite::params![brain_name, &candidate_path],
                |row| row.get(0),
            )
            .ok();
        if let Some(p) = found {
            return (Some(brain_name.to_string()), Some(p), None);
        }
        return (None, None, Some(target.to_string()));
    }

    // Bare name -- look up by `name` column.
    let found: Option<String> = conn
        .query_row(
            "SELECT path FROM brain_fts WHERE brain = ?1 AND name = ?2 LIMIT 1",
            rusqlite::params![brain_name, trimmed],
            |row| row.get(0),
        )
        .ok();
    if let Some(p) = found {
        return (Some(brain_name.to_string()), Some(p), None);
    }
    (None, None, Some(target.to_string()))
}

/// Remove a file from all database tables (FTS, files, dream_log, cross_links, term_weights, doc_norms).
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

    // Clear wikilinks where this file is the SOURCE.
    //
    // Target-side rows are intentionally preserved: leaving them in place
    // means other memories that linked to this file will surface as broken
    // outgoing references to UI consumers, instead of silently disappearing.
    // (See DW-2.4.)
    conn.execute(
        "DELETE FROM links WHERE brain = ?1 AND src_path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete links (src): {e}"))?;

    conn.execute(
        "DELETE FROM tags WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain_name, rel_path],
    )
    .map_err(|e| format!("delete tags: {e}"))?;

    tfidf::remove_weights(conn, brain_name, rel_path)?;

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
    fn test_dw_1_3_index_file_populates_term_weights() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Need multiple docs so IDF filtering doesn't exclude everything.
        // The last file indexed gets accurate weights since all docs are in the corpus.
        let f1 = create_brain_file(
            &brain_dir,
            "notes/rust.md",
            "---\nname: rust-guide\n---\n\nRust programming language for systems development.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/python.md",
            "---\nname: python-guide\n---\n\nPython scripting language for web applications.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/go.md",
            "---\nname: go-guide\n---\n\nGo programming language for services and networking.",
        );

        index_file(db.conn(), "memories", "notes/rust.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/python.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/go.md", &f3, "ref").unwrap();

        // The last file indexed should have term_weights (accurate DF with full corpus)
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'ref/go.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "index_file should populate term_weights");

        // And doc_norms
        let norm: f64 = db
            .conn()
            .query_row(
                "SELECT norm FROM doc_norms WHERE brain = 'memories' AND path = 'ref/go.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(norm > 0.0, "index_file should populate doc_norms");
    }

    #[test]
    fn test_dw_1_4_remove_file_cleans_term_weights() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Index the doomed file LAST so it gets accurate weights with full corpus
        let f1 = create_brain_file(
            &brain_dir,
            "notes/keeper.md",
            "---\nname: keeper\n---\n\nDifferent content about gardening and botany.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "ref/other.md",
            "---\nname: other\n---\n\nAnother unrelated document about cooking recipes.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "notes/doomed.md",
            "---\nname: doomed\n---\n\nUnique specialized content about algorithms and sorting.",
        );

        index_file(db.conn(), "memories", "notes/keeper.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/other.md", &f2, "ref").unwrap();
        index_file(db.conn(), "memories", "notes/doomed.md", &f3, "notes").unwrap();

        // Verify weights exist for the last-indexed file
        let before: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(before > 0, "doomed file should have term_weights");

        // Remove should clean term_weights and doc_norms
        remove_file(db.conn(), "memories", "notes/doomed.md").unwrap();

        let tw_after: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tw_after, 0, "term_weights should be cleaned on remove");

        let dn_after: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM doc_norms WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dn_after, 0, "doc_norms should be cleaned on remove");
    }

    #[test]
    fn test_dw_1_5_sync_handles_term_weights() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create files with highly distinct content so IDF filtering keeps terms
        // even when indexed incrementally during sync
        create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: alpha\n---\n\nAlgorithms sorting mergesort quicksort heapsort.",
        );
        create_brain_file(
            &brain_dir,
            "notes/b.md",
            "---\nname: beta\n---\n\nGardening botany photosynthesis chlorophyll plants.",
        );
        create_brain_file(
            &brain_dir,
            "ref/c.md",
            "---\nname: gamma\n---\n\nCooking baking fermentation sourdough recipes.",
        );

        let brain = db.config().primary_brain().clone();
        let (on_disk, indexed, _removed) = sync_brain(db.conn(), &brain).unwrap();
        assert_eq!(on_disk, 3);
        assert_eq!(indexed, 3);

        // At least some files should have term_weights (later-indexed files get
        // accurate DF ratios; the first file indexed when corpus=1 may have all
        // terms filtered since DF ratio = 1/1 = 100% > 50%)
        let tw_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(DISTINCT path) FROM term_weights WHERE brain = 'memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tw_count >= 2, "sync should populate term_weights for most files, got {tw_count}");

        // doc_norms should be populated for files that have weights
        let dn_count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM doc_norms WHERE brain = 'memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(dn_count >= 2, "sync should populate doc_norms, got {dn_count}");

        // Delete a file from disk and re-sync
        std::fs::remove_file(brain_dir.join("ref/c.md")).unwrap();
        let (_on_disk2, _indexed2, removed2) = sync_brain(db.conn(), &brain).unwrap();
        assert_eq!(removed2, 1);

        // Deleted file's term_weights and doc_norms should be gone
        let tw_after: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'ref/c.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tw_after, 0, "sync should clean term_weights for removed files");

        let dn_after: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM doc_norms WHERE brain = 'memories' AND path = 'ref/c.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dn_after, 0, "sync should clean doc_norms for removed files");
    }

    // ----- DW-2.3: index_file populates links + tags -----

    #[test]
    fn test_dw_2_3_index_file_populates_tags() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/tagged.md",
            "---\nname: tagged\n---\n\nThis has #rust and #foo-bar tags.",
        );
        index_file(db.conn(), "memories", "notes/tagged.md", &f, "notes").unwrap();

        let mut tags: Vec<String> = db
            .conn()
            .prepare("SELECT tag FROM tags WHERE brain = 'memories' AND path = 'notes/tagged.md'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        tags.sort();
        assert_eq!(tags, vec!["foo-bar".to_string(), "rust".to_string()]);
    }

    #[test]
    fn test_dw_2_3_index_file_populates_links() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/source.md",
            "---\nname: source\n---\n\nSee [[Unresolved Target]] reference.",
        );
        index_file(db.conn(), "memories", "notes/source.md", &f, "notes").unwrap();

        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM links WHERE brain = 'memories' AND src_path = 'notes/source.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Unresolved -> target_brain/target_path NULL, name set
        let unresolved: String = db
            .conn()
            .query_row(
                "SELECT target_name_unresolved FROM links WHERE brain = 'memories' AND src_path = 'notes/source.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved, "Unresolved Target");
    }

    #[test]
    fn test_dw_2_3_index_file_resolves_within_brain_link() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Index target FIRST so the source's link can resolve to it
        let f_target = create_brain_file(
            &brain_dir,
            "notes/target.md",
            "---\nname: target-note\n---\n\nTarget body.",
        );
        index_file(db.conn(), "memories", "notes/target.md", &f_target, "notes").unwrap();

        let f_src = create_brain_file(
            &brain_dir,
            "notes/source.md",
            "---\nname: source\n---\n\nLinks to [[target-note]] by name.",
        );
        index_file(db.conn(), "memories", "notes/source.md", &f_src, "notes").unwrap();

        let row: (Option<String>, Option<String>, Option<String>) = db
            .conn()
            .query_row(
                "SELECT target_brain, target_path, target_name_unresolved FROM links \
                 WHERE brain = 'memories' AND src_path = 'notes/source.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("memories"));
        assert_eq!(row.1.as_deref(), Some("notes/target.md"));
        assert!(row.2.is_none(), "resolved link should have NULL unresolved name, got {:?}", row.2);
    }

    #[test]
    fn test_dw_2_3_re_index_replaces_links_and_tags() {
        // Editing a memory should not accumulate stale links/tags rows.
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/edit.md",
            "---\nname: edit\n---\n\n#first [[FirstLink]]",
        );
        index_file(db.conn(), "memories", "notes/edit.md", &f, "notes").unwrap();

        // Overwrite with different links/tags
        std::fs::write(&f, "---\nname: edit\n---\n\n#second [[SecondLink]]").unwrap();
        index_file(db.conn(), "memories", "notes/edit.md", &f, "notes").unwrap();

        let tags: Vec<String> = db
            .conn()
            .prepare("SELECT tag FROM tags WHERE brain = 'memories' AND path = 'notes/edit.md'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(tags, vec!["second".to_string()]);

        let unresolved: Vec<String> = db
            .conn()
            .prepare("SELECT target_name_unresolved FROM links WHERE brain = 'memories' AND src_path = 'notes/edit.md'")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(unresolved, vec!["SecondLink".to_string()]);
    }

    // ----- DW-2.4: remove_file source-side cleanup, target-side preserved -----

    #[test]
    fn test_dw_2_4_remove_file_deletes_source_links_and_tags() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/doomed.md",
            "---\nname: doomed\n---\n\n#tagx and [[Other]]",
        );
        index_file(db.conn(), "memories", "notes/doomed.md", &f, "notes").unwrap();

        // Pre-condition: rows exist
        let pre_links: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM links WHERE brain = 'memories' AND src_path = 'notes/doomed.md'",
                [], |row| row.get(0),
            )
            .unwrap();
        assert!(pre_links > 0);

        remove_file(db.conn(), "memories", "notes/doomed.md").unwrap();

        let post_links: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM links WHERE brain = 'memories' AND src_path = 'notes/doomed.md'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(post_links, 0);

        let post_tags: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM tags WHERE brain = 'memories' AND path = 'notes/doomed.md'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(post_tags, 0);
    }

    #[test]
    fn test_dw_2_4_remove_file_preserves_target_side_links() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Index target first so source link resolves
        let f_target = create_brain_file(
            &brain_dir,
            "notes/target.md",
            "---\nname: target-note\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/target.md", &f_target, "notes").unwrap();

        let f_src = create_brain_file(
            &brain_dir,
            "notes/source.md",
            "---\nname: source\n---\n\nRefers to [[target-note]]",
        );
        index_file(db.conn(), "memories", "notes/source.md", &f_src, "notes").unwrap();

        // Now remove the TARGET. The link row from source -> target_path
        // should remain so source's reference shows as a broken link.
        remove_file(db.conn(), "memories", "notes/target.md").unwrap();

        let preserved: i32 = db.conn().query_row(
            "SELECT COUNT(*) FROM links WHERE brain = 'memories' AND src_path = 'notes/source.md' \
             AND target_path = 'notes/target.md'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(
            preserved, 1,
            "target-side link row should be preserved on target removal"
        );
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
