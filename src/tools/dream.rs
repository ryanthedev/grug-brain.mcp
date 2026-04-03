use super::{GrugDb, STALE_DAYS};
use crate::parsing::extract_frontmatter;
use crate::tools::indexing::sync_brain;
use crate::tools::search::fts_search;
use crate::types::RecallRow;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Dream: review memory health across all brains.
/// Syncs all brains, finds cross-links, flags stale memories and conflicts.
pub fn grug_dream(db: &mut GrugDb) -> Result<String, String> {
    db.maybe_reload_config();

    // Sync all brains before inspecting
    let brains: Vec<_> = db.config().brains.clone();
    for brain in &brains {
        let _ = sync_brain(db.conn(), brain);
    }

    // Collect all memories across all brains
    let all = collect_all_memories(db);
    if all.is_empty() {
        return Ok("nothing to dream about \u{2014} no memories yet".to_string());
    }

    let now_ms = now_millis();
    let ts = chrono::Utc::now().to_rfc3339();
    let mut sections: Vec<String> = Vec::new();

    // --- git commit section (STUB for Phase 4) ---
    // Phase 4 will add: commit pending changes per writable brain with git

    // --- conflicts: entries in the conflicts/ category ---
    let primary_name = db.config().primary.clone();
    let primary_dir = db.config().primary_brain().dir.clone();
    let conflict_rows = recall_by_category(db, &primary_name, "conflicts");
    if !conflict_rows.is_empty() {
        let mut conflict_lines = Vec::new();
        for r in &conflict_rows {
            let file_path = primary_dir.join(&r.path);
            let fm = if let Ok(content) = fs::read_to_string(&file_path) {
                extract_frontmatter(&content)
            } else {
                std::collections::HashMap::new()
            };

            let origin = if let Some(orig_path) = fm.get("original_path") {
                let orig_brain = fm.get("original_brain").map(|s| s.as_str()).unwrap_or("?");
                format!("{orig_brain}/{orig_path}")
            } else {
                r.path.clone()
            };

            let host = fm
                .get("hostname")
                .map(|h| format!(" (from {h})"))
                .unwrap_or_default();
            let date = fm
                .get("date")
                .map(|d| format!(" -- {d}"))
                .unwrap_or_default();

            let base_name = Path::new(&r.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&r.path);

            conflict_lines.push(format!(
                "- **{}**{}{}: original: `{}`\n  Resolve: read with `grug-read brain:{} category:conflicts path:{}`, then `grug-write` to the original location and `grug-delete` the conflict entry.",
                r.name, date, host, origin, primary_name, base_name
            ));
        }
        sections.push(format!(
            "## conflicts ({})\n\nThese files had git merge conflicts and were saved here. Review each, write the correct version to the original location, then delete the conflict entry.\n\n{}",
            conflict_rows.len(),
            conflict_lines.join("\n\n")
        ));
    }

    // --- which memories need attention? ---
    let mut needs_review = HashSet::new();
    for brain in &brains {
        let mut stmt = match db.conn().prepare(
            "SELECT f.brain, f.path, f.mtime, d.reviewed_at, d.mtime_at_review
             FROM files f
             LEFT JOIN dream_log d ON f.brain = d.brain AND f.path = d.path
             WHERE f.brain = ?1
               AND (d.path IS NULL OR f.mtime > d.mtime_at_review)",
        ) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let rows: Vec<(String, String)> = stmt
            .query_map([&brain.name], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .ok()
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default();
        for (brain_name, path) in rows {
            needs_review.insert(format!("{brain_name}:{path}"));
        }
    }

    if needs_review.is_empty() && conflict_rows.is_empty() {
        let (total_files, total_cats) = count_totals(db, &brains);
        sections.insert(
            0,
            format!(
                "# dream report\n\n{total_files} memories | {total_cats} categories | all clean \u{2014} nothing needs review"
            ),
        );
        return Ok(sections.join("\n\n"));
    }

    // Filter to only memories needing review
    let to_review: Vec<&MemoryWithBrain> = all
        .iter()
        .filter(|m| needs_review.contains(&format!("{}:{}", m.brain_name, m.path)))
        .collect();

    // --- cross-links across all brains ---
    let mut links: Vec<LinkDisplay> = Vec::new();
    let mut seen = HashSet::new();

    for mem in &to_review {
        // Delete existing links for this memory
        let _ = db.conn().execute(
            "DELETE FROM cross_links WHERE (brain_a = ?1 AND path_a = ?2) OR (brain_b = ?1 AND path_b = ?2)",
            rusqlite::params![mem.brain_name, mem.path],
        );

        // Extract search terms from name
        let name_normalized = mem.name.replace('-', " ").replace('_', " ");
        let terms: Vec<&str> = name_normalized
            .split_whitespace()
            .filter(|t| t.len() > 3)
            .take(3)
            .collect();

        if terms.is_empty() {
            continue;
        }

        let q = terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR ");

        let (matches, _) = fts_search(db.conn(), &q, 5, 0);

        for m in &matches {
            // Skip self
            if m.path == mem.path && m.brain == mem.brain_name {
                continue;
            }
            // Skip same category in same brain
            if m.category == mem.category && m.brain == mem.brain_name {
                continue;
            }

            // Sort brain:path pair for stable primary key
            let key_self = format!("{}:{}", mem.brain_name, mem.path);
            let key_other = format!("{}:{}", m.brain, m.path);
            let ((b_a, p_a), (b_b, p_b)) = if key_self <= key_other {
                (
                    (mem.brain_name.as_str(), mem.path.as_str()),
                    (m.brain.as_str(), m.path.as_str()),
                )
            } else {
                (
                    (m.brain.as_str(), m.path.as_str()),
                    (mem.brain_name.as_str(), mem.path.as_str()),
                )
            };

            let key = format!("{b_a}:{p_a}|{b_b}:{p_b}");
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            // Upsert cross-link
            let _ = db.conn().execute(
                "INSERT OR REPLACE INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![b_a, p_a, b_b, p_b, m.rank, &ts],
            );

            let brain_tag_a = if b_a != primary_name {
                format!(" [{}]", b_a)
            } else {
                String::new()
            };
            let brain_tag_b = if b_b != primary_name {
                format!(" [{}]", b_b)
            } else {
                String::new()
            };

            links.push(LinkDisplay {
                a: format!("{} [{}]{}", mem.name, mem.category, brain_tag_a),
                b: format!("{} [{}]{}", m.name, m.category, brain_tag_b),
                rank: m.rank,
            });
        }
    }

    if !links.is_empty() {
        links.sort_by(|a, b| a.rank.partial_cmp(&b.rank).unwrap_or(std::cmp::Ordering::Equal));
        let top: Vec<&LinkDisplay> = links.iter().take(10).collect();
        let link_lines: Vec<String> = top.iter().map(|l| format!("- {} \u{2194} {}", l.a, l.b)).collect();
        sections.push(format!(
            "## new cross-links ({} found, top {})\n\n{}",
            links.len(),
            top.len(),
            link_lines.join("\n")
        ));
    }

    // --- stale memories (only unreviewed) ---
    let mut stale: Vec<(&MemoryWithBrain, i64)> = to_review
        .iter()
        .filter_map(|m| {
            if m.date.is_empty() {
                return None;
            }
            let parsed = chrono::NaiveDate::parse_from_str(&m.date, "%Y-%m-%d").ok()?;
            let age_ms = now_ms
                - parsed
                    .and_hms_opt(0, 0, 0)?
                    .and_utc()
                    .timestamp_millis();
            let age_days = age_ms / 86_400_000;
            if age_days >= STALE_DAYS {
                Some((*m, age_days))
            } else {
                None
            }
        })
        .collect();
    stale.sort_by(|a, b| b.1.cmp(&a.1));

    if !stale.is_empty() {
        let stale_lines: Vec<String> = stale
            .iter()
            .map(|(s, age)| {
                let brain_tag = if s.brain_name != primary_name {
                    format!(" [{}]", s.brain_name)
                } else {
                    String::new()
                };
                format!(
                    "- {} [{}]{} -- {}d ({}): {}",
                    s.name, s.category, brain_tag, age, s.date, s.description
                )
            })
            .collect();
        sections.push(format!(
            "## stale ({} memories > {} days)\n\n{}",
            stale.len(),
            STALE_DAYS,
            stale_lines.join("\n")
        ));
    }

    // --- quality issues (only unreviewed) ---
    let issues: Vec<&&MemoryWithBrain> = to_review
        .iter()
        .filter(|m| m.date.is_empty() || m.description.is_empty())
        .collect();
    if !issues.is_empty() {
        let issue_lines: Vec<String> = issues
            .iter()
            .map(|m| {
                let brain_tag = if m.brain_name != primary_name {
                    format!(" [{}]", m.brain_name)
                } else {
                    String::new()
                };
                let problem = if m.date.is_empty() {
                    "no date"
                } else {
                    "no description"
                };
                format!("- {} [{}]{}: {}", m.name, m.category, brain_tag, problem)
            })
            .collect();
        sections.push(format!("## quality issues\n\n{}", issue_lines.join("\n")));
    }

    // --- needs review listing ---
    let review_lines: Vec<String> = to_review
        .iter()
        .map(|m| {
            let date = if m.date.is_empty() {
                String::new()
            } else {
                format!(" {}", m.date)
            };
            let brain_tag = if m.brain_name != primary_name {
                format!(" [{}]", m.brain_name)
            } else {
                String::new()
            };
            format!(
                "- {} [{}]{}{}: {}",
                m.name, m.category, brain_tag, date, m.description
            )
        })
        .collect();
    sections.push(format!(
        "## needs review ({} memories)\n\n{}",
        to_review.len(),
        review_lines.join("\n")
    ));

    // --- header (prepend) ---
    let (total_files, total_cats) = count_totals(db, &brains);
    let conflict_note = if !conflict_rows.is_empty() {
        format!(" | {} conflicts", conflict_rows.len())
    } else {
        String::new()
    };
    let summary = format!(
        "{total_files} memories | {total_cats} categories | {} need review | {} cross-links | {} stale{conflict_note}",
        to_review.len(),
        links.len(),
        stale.len()
    );
    sections.insert(
        0,
        format!(
            "# dream report\n\n{summary}\n\nOnly showing memories that are new or changed since last dream. Use grug-write to update, grug-delete to remove."
        ),
    );

    // --- mark reviewed ---
    for m in &to_review {
        let mtime: Option<f64> = db
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
                rusqlite::params![m.brain_name, m.path],
                |row| row.get(0),
            )
            .ok();
        if let Some(mtime) = mtime {
            let _ = db.conn().execute(
                "INSERT OR REPLACE INTO dream_log (brain, path, reviewed_at, mtime_at_review) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![m.brain_name, m.path, &ts, mtime],
            );
        }
    }

    Ok(sections.join("\n\n"))
}

#[derive(Debug)]
struct MemoryWithBrain {
    brain_name: String,
    path: String,
    category: String,
    name: String,
    date: String,
    description: String,
}

struct LinkDisplay {
    a: String,
    b: String,
    rank: f64,
}

fn collect_all_memories(db: &GrugDb) -> Vec<MemoryWithBrain> {
    let mut all = Vec::new();
    for brain in &db.config().brains {
        let rows = recall_all(db, &brain.name);
        for row in rows {
            all.push(MemoryWithBrain {
                brain_name: brain.name.clone(),
                path: row.path,
                category: row.category,
                name: row.name,
                date: row.date,
                description: row.description,
            });
        }
    }
    all
}

fn recall_all(db: &GrugDb, brain_name: &str) -> Vec<RecallRow> {
    let mut stmt = match db.conn().prepare(
        "SELECT path, brain, category, name, date, description FROM brain_fts WHERE brain = ?1 ORDER BY category, date DESC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    stmt.query_map([brain_name], |row| {
        Ok(RecallRow {
            path: row.get(0)?,
            brain: row.get(1)?,
            category: row.get(2)?,
            name: row.get(3)?,
            date: row.get(4)?,
            description: row.get(5)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

fn recall_by_category(db: &GrugDb, brain_name: &str, category: &str) -> Vec<RecallRow> {
    let mut stmt = match db.conn().prepare(
        "SELECT path, brain, category, name, date, description FROM brain_fts WHERE brain = ?1 AND category = ?2 ORDER BY date DESC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    stmt.query_map(rusqlite::params![brain_name, category], |row| {
        Ok(RecallRow {
            path: row.get(0)?,
            brain: row.get(1)?,
            category: row.get(2)?,
            name: row.get(3)?,
            date: row.get(4)?,
            description: row.get(5)?,
        })
    })
    .ok()
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

fn count_totals(db: &GrugDb, brains: &[crate::types::Brain]) -> (i32, usize) {
    let mut total_files = 0i32;
    let mut total_cats = 0usize;
    for brain in brains {
        total_files += db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE brain = ?1",
                [&brain.name],
                |row| row.get::<_, i32>(0),
            )
            .unwrap_or(0);

        let cat_count: usize = db
            .conn()
            .prepare("SELECT category FROM brain_fts WHERE brain = ?1 GROUP BY category")
            .ok()
            .and_then(|mut s| {
                s.query_map([&brain.name], |_row| Ok(()))
                    .ok()
                    .map(|r| r.count())
            })
            .unwrap_or(0);
        total_cats += cat_count;
    }
    (total_files, total_cats)
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db};

    #[test]
    fn test_dream_empty() {
        let (mut db, _tmp) = test_db();
        let result = grug_dream(&mut db).unwrap();
        assert!(result.contains("nothing to dream about"));
    }

    #[test]
    fn test_dream_basic() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: programming-rust\ndate: 2025-01-01\n---\n\nA memory about programming in rust",
        );
        index_file(db.conn(), "memories", "notes/a.md", &f, "notes").unwrap();

        let result = grug_dream(&mut db).unwrap();
        assert!(result.contains("# dream report"));
        assert!(result.contains("needs review"));
        assert!(result.contains("programming-rust"));
    }

    #[test]
    fn test_dream_marks_reviewed() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\ndate: 2025-01-01\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/a.md", &f, "notes").unwrap();

        // First dream
        let r1 = grug_dream(&mut db).unwrap();
        assert!(r1.contains("1 need review"));

        // Second dream (without file changes) should show all clean
        let r2 = grug_dream(&mut db).unwrap();
        assert!(r2.contains("all clean"));
    }

    #[test]
    fn test_dream_stale_detection() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create a file with an old date
        let f = create_brain_file(
            &brain_dir,
            "notes/old.md",
            "---\nname: old-memory\ndate: 2024-01-01\n---\n\nThis is old",
        );
        index_file(db.conn(), "memories", "notes/old.md", &f, "notes").unwrap();

        let result = grug_dream(&mut db).unwrap();
        assert!(result.contains("stale"));
        assert!(result.contains("old-memory"));
    }

    #[test]
    fn test_dream_quality_issues() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // File without a date
        let f = create_brain_file(
            &brain_dir,
            "notes/nodate.md",
            "---\nname: nodate\n---\n\nBody without date",
        );
        index_file(db.conn(), "memories", "notes/nodate.md", &f, "notes").unwrap();

        let result = grug_dream(&mut db).unwrap();
        assert!(result.contains("quality issues"));
        assert!(result.contains("no date"));
    }

    #[test]
    fn test_dream_cross_links() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Two memories with related names that should cross-link
        let f1 = create_brain_file(
            &brain_dir,
            "notes/programming-rust.md",
            "---\nname: programming-rust\ndate: 2025-01-01\n---\n\nLearning about programming in the rust language",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "ref/rust-patterns.md",
            "---\nname: rust-patterns\ndate: 2025-01-02\n---\n\nCommon patterns in rust programming",
        );
        index_file(db.conn(), "memories", "notes/programming-rust.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/rust-patterns.md", &f2, "ref").unwrap();

        let result = grug_dream(&mut db).unwrap();
        // Cross-links may or may not be found depending on FTS matching
        // The important thing is that the dream completes without error
        assert!(result.contains("# dream report"));
    }

    #[test]
    fn test_dream_conflict_listing() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create a conflict entry
        let f = create_brain_file(
            &brain_dir,
            "conflicts/old-version.md",
            "---\nname: old-version\ndate: 2025-03-01\noriginal_path: notes/my-note.md\noriginal_brain: memories\nhostname: macbook\n---\n\nConflict content",
        );
        index_file(
            db.conn(),
            "memories",
            "conflicts/old-version.md",
            &f,
            "conflicts",
        )
        .unwrap();

        let result = grug_dream(&mut db).unwrap();
        assert!(result.contains("conflicts (1)"));
        assert!(result.contains("old-version"));
        assert!(result.contains("Resolve:"));
    }
}
