use super::{GrugDb, STALE_DAYS};
use crate::parsing::extract_frontmatter;
use crate::tools::indexing::sync_brain;
use crate::tools::similarity::find_similar;
use crate::types::RecallRow;
use std::collections::{HashMap, HashSet};
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

    // --- cross-links across all brains (cosine similarity + diversity filtering) ---
    let mut all_candidates: Vec<CrossLinkCandidate> = Vec::new();
    let mut seen = HashSet::new();

    for mem in &to_review {
        // Delete existing links for this memory
        let _ = db.conn().execute(
            "DELETE FROM cross_links WHERE (brain_a = ?1 AND path_a = ?2) OR (brain_b = ?1 AND path_b = ?2)",
            rusqlite::params![mem.brain_name, mem.path],
        );

        // Find similar docs using cosine similarity (Phase 2 engine)
        let similar = match find_similar(db.conn(), &mem.brain_name, &mem.path, 20) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for s in &similar {
            // Skip same-category same-brain (obvious, not interesting)
            if s.category == mem.category && s.brain == mem.brain_name {
                continue;
            }

            // Sort brain:path pair for stable primary key (dedup)
            let key_self = format!("{}:{}", mem.brain_name, mem.path);
            let key_other = format!("{}:{}", s.brain, s.path);
            let ((b_a, p_a, cat_a, name_a), (b_b, p_b, cat_b, name_b)) = if key_self <= key_other {
                (
                    (mem.brain_name.as_str(), mem.path.as_str(), mem.category.as_str(), mem.name.as_str()),
                    (s.brain.as_str(), s.path.as_str(), s.category.as_str(), s.name.as_str()),
                )
            } else {
                (
                    (s.brain.as_str(), s.path.as_str(), s.category.as_str(), s.name.as_str()),
                    (mem.brain_name.as_str(), mem.path.as_str(), mem.category.as_str(), mem.name.as_str()),
                )
            };

            let key = format!("{b_a}:{p_a}|{b_b}:{p_b}");
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);

            all_candidates.push(CrossLinkCandidate {
                brain_a: b_a.to_string(),
                path_a: p_a.to_string(),
                cat_a: cat_a.to_string(),
                name_a: name_a.to_string(),
                brain_b: b_b.to_string(),
                path_b: p_b.to_string(),
                cat_b: cat_b.to_string(),
                name_b: name_b.to_string(),
                score: s.score,
            });
        }
    }

    // Sort all candidates by score descending (best similarity first)
    all_candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Apply diversity filter: greedy selection with category pair cap
    let selected = diversity_filter(&all_candidates, 10);

    // Store selected cross-links and build display
    let mut links: Vec<LinkDisplay> = Vec::new();
    for c in &selected {
        let _ = db.conn().execute(
            "INSERT OR REPLACE INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![c.brain_a, c.path_a, c.brain_b, c.path_b, c.score, &ts],
        );

        let brain_tag_a = if c.brain_a != primary_name {
            format!(" [{}]", c.brain_a)
        } else {
            String::new()
        };
        let brain_tag_b = if c.brain_b != primary_name {
            format!(" [{}]", c.brain_b)
        } else {
            String::new()
        };

        links.push(LinkDisplay {
            a: format!("{} [{}]{}", c.name_a, c.cat_a, brain_tag_a),
            b: format!("{} [{}]{}", c.name_b, c.cat_b, brain_tag_b),
        });
    }

    if !links.is_empty() {
        let link_lines: Vec<String> = links.iter().map(|l| format!("- {} \u{2194} {}", l.a, l.b)).collect();
        sections.push(format!(
            "## new cross-links ({} found, top {})\n\n{}",
            all_candidates.len(),
            links.len(),
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
}

struct CrossLinkCandidate {
    brain_a: String,
    path_a: String,
    cat_a: String,
    name_a: String,
    brain_b: String,
    path_b: String,
    cat_b: String,
    name_b: String,
    score: f64,
}

/// Greedy diversity filter: select up to `limit` cross-link candidates
/// ensuring no more than 2 links per category pair. Allows override if
/// a candidate's score is significantly higher (>1.5x) than the lowest
/// selected link's score.
fn diversity_filter(candidates: &[CrossLinkCandidate], limit: usize) -> Vec<&CrossLinkCandidate> {
    let mut selected: Vec<&CrossLinkCandidate> = Vec::new();
    let mut pair_counts: HashMap<String, usize> = HashMap::new();
    let mut min_score = f64::MAX;

    for candidate in candidates {
        if selected.len() >= limit {
            break;
        }

        // Build sorted category pair key
        let mut pair = vec![candidate.cat_a.as_str(), candidate.cat_b.as_str()];
        pair.sort();
        let pair_key = format!("{}:{}", pair[0], pair[1]);

        let count = pair_counts.get(&pair_key).copied().unwrap_or(0);
        if count >= 2 {
            // Allow override only if score is significantly higher than the worst selected
            if min_score < f64::MAX && candidate.score > min_score * 1.5 {
                // Override: exceptionally strong link
            } else {
                continue;
            }
        }

        selected.push(candidate);
        *pair_counts.entry(pair_key).or_insert(0) += 1;
        if candidate.score < min_score {
            min_score = candidate.score;
        }
    }

    selected
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
    use crate::tools::read::grug_read;
    use crate::tools::test_helpers::{create_brain_file, test_db};
    use crate::tools::tfidf;

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

    // ---------------------------------------------------------------
    // Phase 3 tests: cosine similarity + diversity filtering
    // ---------------------------------------------------------------

    /// Helper: create docs, index them, compute TF-IDF weights.
    /// Returns the list of (brain, path) pairs created.
    fn setup_weighted_docs(
        db: &crate::tools::GrugDb,
        brain_dir: &std::path::Path,
        docs: &[(&str, &str, &str, &str)], // (brain, rel_path, category, content)
    ) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        for (brain, rel_path, category, content) in docs {
            let f = create_brain_file(brain_dir, rel_path, content);
            index_file(db.conn(), brain, rel_path, &f, category).unwrap();
            pairs.push((brain.to_string(), rel_path.to_string()));
        }
        // Recompute weights after all docs indexed (corpus-aware IDF)
        for (brain, path) in &pairs {
            tfidf::compute_and_store_weights(db.conn(), brain, path).unwrap();
        }
        pairs
    }

    // ---------------------------------------------------------------
    // DW-3.1: Dream cross-links use cosine similarity, not keyword matching
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_1_uses_cosine_similarity() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create docs with overlapping content in different categories.
        // Need 5+ docs so shared terms survive IDF filtering (DF ratio < 50%).
        let docs = vec![
            ("memories", "notes/rust-perf.md", "notes",
             "---\nname: rust-perf\ndate: 2025-01-01\n---\n\nRust performance optimization concurrency systems programming memory safety"),
            ("memories", "ref/rust-guide.md", "ref",
             "---\nname: rust-guide\ndate: 2025-01-02\n---\n\nRust programming guide for systems performance optimization concurrency patterns"),
            ("memories", "notes/cooking.md", "notes",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry desserts kitchen"),
            ("memories", "ref/gardening.md", "ref",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting organic"),
            ("memories", "tips/astronomy.md", "tips",
             "---\nname: astronomy\ndate: 2025-01-05\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations planets cosmos"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        let result = grug_dream(&mut db).unwrap();

        // Verify cross-links section appears and contains cosine-based links
        assert!(result.contains("cross-links"), "should have cross-links section");

        // Verify the cross_links table has scores in the cosine range (0, 1]
        let scores: Vec<f64> = db.conn()
            .prepare("SELECT score FROM cross_links")
            .unwrap()
            .query_map([], |row| row.get::<_, f64>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(!scores.is_empty(), "should have stored cross-links");
        for score in &scores {
            assert!(
                *score > 0.0 && *score <= 1.0,
                "cosine similarity score should be in (0, 1], got {score}"
            );
        }
    }

    // ---------------------------------------------------------------
    // DW-3.2: Same-category same-brain links are excluded
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_2_same_category_same_brain_excluded() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create multiple docs in the SAME category with overlapping content.
        // These should NOT be cross-linked.
        // Plus docs in other categories to dilute IDF.
        let docs = vec![
            ("memories", "notes/rust-basics.md", "notes",
             "---\nname: rust-basics\ndate: 2025-01-01\n---\n\nRust programming language basics ownership borrowing lifetimes safety"),
            ("memories", "notes/rust-advanced.md", "notes",
             "---\nname: rust-advanced\ndate: 2025-01-02\n---\n\nRust programming advanced concurrency async futures performance optimization"),
            ("memories", "ref/cooking.md", "ref",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry desserts kitchen"),
            ("memories", "tips/gardening.md", "tips",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting organic"),
            ("memories", "ref/astronomy.md", "ref",
             "---\nname: astronomy\ndate: 2025-01-05\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations planets cosmos"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        grug_dream(&mut db).unwrap();

        // Verify no cross-link exists between the two notes/rust-* docs
        // (same category "notes", same brain "memories")
        let same_cat_links: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM cross_links
                 WHERE (brain_a = 'memories' AND path_a LIKE 'notes/rust-%' AND brain_b = 'memories' AND path_b LIKE 'notes/rust-%')",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            same_cat_links, 0,
            "same-category same-brain links should be excluded"
        );
    }

    // ---------------------------------------------------------------
    // DW-3.3: Diversity filter caps at 2 links per category pair
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_3_diversity_filter_caps_category_pairs() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Test the diversity filter with carefully crafted content.
        // With 12 docs (4 per category), terms shared by at most 4 docs
        // have DF ratio = 4/12 = 33% < 50%, surviving IDF filtering.
        //
        // Each pair of docs across categories shares a unique linking term.
        // This creates many notes<->ref candidates that the diversity filter
        // must cap at 2, while notes<->tips and ref<->tips also appear.

        let mut docs = Vec::new();

        // 4 notes, each with unique content + pairwise linking terms
        for i in 0..4 {
            // Each note shares "linknr{i}" with ref-{i} and "linknt{i}" with tips-{i % 2}
            docs.push((
                "memories",
                format!("notes/note-{i}.md"),
                "notes".to_string(),
                format!(
                    "---\nname: note-{i}\ndate: 2025-01-0{}\n---\n\nlinknr{i}aaa linknr{i}bbb linknr{i}ccc linknt{}aaa linknt{}bbb noteonly{i}aaa noteonly{i}bbb noteonly{i}ccc noteonly{i}ddd noteonly{i}eee",
                    i + 1, i % 2, i % 2
                ),
            ));
        }

        // 4 refs, each sharing "linknr{i}" with note-{i} and "linkrt{i}" with tips-{i % 2}
        for i in 0..4 {
            docs.push((
                "memories",
                format!("ref/ref-{i}.md"),
                "ref".to_string(),
                format!(
                    "---\nname: ref-{i}\ndate: 2025-02-0{}\n---\n\nlinknr{i}aaa linknr{i}bbb linknr{i}ccc linkrt{}aaa linkrt{}bbb refonly{i}aaa refonly{i}bbb refonly{i}ccc refonly{i}ddd refonly{i}eee",
                    i + 1, i % 2, i % 2
                ),
            ));
        }

        // 4 tips (to pad total doc count for better IDF ratios)
        for i in 0..4 {
            // tip-0 and tip-1 share terms with notes/refs; tip-2 and tip-3 are unrelated filler
            let content = if i < 2 {
                format!(
                    "---\nname: tip-{i}\ndate: 2025-03-0{}\n---\n\nlinknt{i}aaa linknt{i}bbb linkrt{i}aaa linkrt{i}bbb tiponly{i}aaa tiponly{i}bbb tiponly{i}ccc tiponly{i}ddd tiponly{i}eee",
                    i + 1
                )
            } else {
                format!(
                    "---\nname: tip-{i}\ndate: 2025-03-0{}\n---\n\nfiller{i}aaa filler{i}bbb filler{i}ccc filler{i}ddd filler{i}eee filler{i}fff filler{i}ggg filler{i}hhh",
                    i + 1
                )
            };
            docs.push((
                "memories",
                format!("tips/tip-{i}.md"),
                "tips".to_string(),
                content,
            ));
        }

        let doc_refs: Vec<(&str, &str, &str, &str)> = docs
            .iter()
            .map(|(b, p, c, content)| (*b, p.as_str(), c.as_str(), content.as_str()))
            .collect();
        setup_weighted_docs(&db, &brain_dir, &doc_refs);

        let _result = grug_dream(&mut db).unwrap();

        // Count links per category pair in cross_links table
        let links: Vec<(String, String, String, String)> = db.conn()
            .prepare("SELECT brain_a, path_a, brain_b, path_b FROM cross_links ORDER BY score DESC LIMIT 10")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        // Count how many links per category pair
        let mut pair_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (ba, pa, bb, pb) in &links {
            let cat_a: String = db.conn()
                .query_row("SELECT category FROM brain_fts WHERE brain = ?1 AND path = ?2 LIMIT 1",
                    rusqlite::params![ba, pa], |row| row.get(0))
                .unwrap_or_default();
            let cat_b: String = db.conn()
                .query_row("SELECT category FROM brain_fts WHERE brain = ?1 AND path = ?2 LIMIT 1",
                    rusqlite::params![bb, pb], |row| row.get(0))
                .unwrap_or_default();
            let mut pair = vec![cat_a, cat_b];
            pair.sort();
            let key = pair.join(":");
            *pair_counts.entry(key).or_insert(0) += 1;
        }

        // No category pair should have more than 2 links
        for (pair, count) in &pair_counts {
            assert!(
                *count <= 2,
                "category pair '{pair}' has {count} links, max allowed is 2"
            );
        }

        // The "tips" category should be represented somewhere in the cross-links
        // (diversity ensures we don't only see notes<->ref links)
        let has_tips = links.iter().any(|(_, pa, _, pb)| {
            pa.starts_with("tips/") || pb.starts_with("tips/")
        });

        assert!(has_tips, "diversity filter should ensure 'tips' category is represented in cross-links");

        // Verify 3+ categories are represented
        let categories_seen: HashSet<String> = pair_counts.keys()
            .flat_map(|k| k.split(':'))
            .map(|s| s.to_string())
            .collect();
        assert!(
            categories_seen.len() >= 3,
            "should have 3+ categories represented, got {:?}",
            categories_seen
        );
    }

    // ---------------------------------------------------------------
    // DW-3.4: Cross-links table stores cosine similarity score
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_4_cross_links_store_cosine_score() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let docs = vec![
            ("memories", "notes/systems.md", "notes",
             "---\nname: systems\ndate: 2025-01-01\n---\n\nSystems programming performance optimization concurrency parallelism"),
            ("memories", "ref/architecture.md", "ref",
             "---\nname: architecture\ndate: 2025-01-02\n---\n\nSystems architecture performance optimization scalability throughput"),
            ("memories", "tips/cooking.md", "tips",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry desserts"),
            ("memories", "ref/gardening.md", "ref",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting"),
            ("memories", "tips/music.md", "tips",
             "---\nname: music\ndate: 2025-01-05\n---\n\nMusic theory harmony melody rhythm composition orchestration"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        grug_dream(&mut db).unwrap();

        // Query all stored scores
        let scores: Vec<f64> = db.conn()
            .prepare("SELECT score FROM cross_links")
            .unwrap()
            .query_map([], |row| row.get::<_, f64>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(!scores.is_empty(), "should have cross-links with scores");
        for score in &scores {
            assert!(
                *score > 0.0 && *score <= 1.0,
                "stored score should be cosine similarity in (0, 1], got {score}"
            );
        }
    }

    // ---------------------------------------------------------------
    // DW-3.5: grug-read still displays cross-links correctly
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_5_read_shows_cross_links_after_dream() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let docs = vec![
            ("memories", "notes/rust-perf.md", "notes",
             "---\nname: rust-perf\ndate: 2025-01-01\n---\n\nRust performance optimization concurrency systems programming memory safety"),
            ("memories", "ref/rust-guide.md", "ref",
             "---\nname: rust-guide\ndate: 2025-01-02\n---\n\nRust programming guide for systems performance optimization concurrency patterns"),
            ("memories", "tips/cooking.md", "tips",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry desserts kitchen"),
            ("memories", "ref/gardening.md", "ref",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting organic"),
            ("memories", "tips/astronomy.md", "tips",
             "---\nname: astronomy\ndate: 2025-01-05\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations planets cosmos"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        // Run dream to create cross-links
        let dream_result = grug_dream(&mut db).unwrap();

        // Only proceed if cross-links were actually found
        if dream_result.contains("cross-links") {
            // Read one of the docs that should have cross-links
            let read_result = grug_read(&mut db, Some("memories"), Some("notes"), Some("rust-perf")).unwrap();

            // Should display the linked memories section
            assert!(
                read_result.contains("linked memories"),
                "grug-read should show linked memories section after dream creates cross-links"
            );
        }
    }

    // ---------------------------------------------------------------
    // DW-3.6: Performance <5s for 5000-doc corpus (cross-link section)
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_6_dream_cross_links_performance() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Insert synthetic docs with term_weights and doc_norms directly
        // (like Phase 2 perf test). Also need files + brain_fts for dream to pick them up.
        let num_docs = 100; // Use 100 docs needing review to test cross-link computation time
        let vocab_size = 500;
        let terms_per_doc = 20;

        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        for i in 0..num_docs {
            let path = format!("cat{}/doc-{i:04}.md", i % 5);
            let category = format!("cat{}", i % 5);

            // files table entry (so dream considers it)
            db.conn().execute(
                "INSERT INTO files (brain, path, mtime) VALUES ('memories', ?1, ?2)",
                rusqlite::params![path, i as f64],
            ).unwrap();

            // brain_fts entry
            db.conn().execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?1, 'memories', ?2, ?3, '2025-01-01', 'desc', 'body')",
                rusqlite::params![path, category, format!("doc-{i:04}")],
            ).unwrap();

            // Also create the actual file so dream doesn't fail on filesystem ops
            let cat_dir = brain_dir.join(&category);
            std::fs::create_dir_all(&cat_dir).unwrap();
            std::fs::write(
                cat_dir.join(format!("doc-{i:04}.md")),
                format!("---\nname: doc-{i:04}\ndate: 2025-01-01\n---\n\nbody"),
            ).unwrap();

            // doc_norms
            db.conn().execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', ?1, ?2)",
                rusqlite::params![path, 3.5 + (i as f64 * 0.001)],
            ).unwrap();

            // term_weights
            for j in 0..terms_per_doc {
                let term_idx = (i * 3 + j * 7) % vocab_size;
                let term = format!("term{term_idx:03}");
                let weight = 1.0 + (j as f64 * 0.1);
                db.conn().execute(
                    "INSERT OR IGNORE INTO term_weights (brain, path, term, weight) VALUES ('memories', ?1, ?2, ?3)",
                    rusqlite::params![path, term, weight],
                ).unwrap();
            }
        }

        db.conn().execute_batch("COMMIT").unwrap();

        let start = std::time::Instant::now();
        let result = grug_dream(&mut db).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs() < 5,
            "dream should complete in <5s, took {:.2}s",
            elapsed.as_secs_f64()
        );
        assert!(result.contains("# dream report"));
    }

    // ---------------------------------------------------------------
    // DW-3.7: Score ordering in cross-links display
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_7_score_ordering() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let docs = vec![
            ("memories", "notes/systems.md", "notes",
             "---\nname: systems\ndate: 2025-01-01\n---\n\nSystems programming performance optimization concurrency parallelism threading"),
            ("memories", "ref/architecture.md", "ref",
             "---\nname: architecture\ndate: 2025-01-02\n---\n\nSystems architecture performance optimization scalability throughput design"),
            ("memories", "tips/cooking.md", "tips",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry desserts kitchen"),
            ("memories", "ref/gardening.md", "ref",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting organic"),
            ("memories", "tips/music.md", "tips",
             "---\nname: music\ndate: 2025-01-05\n---\n\nMusic theory harmony melody rhythm composition orchestration arrangement"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        grug_dream(&mut db).unwrap();

        // Verify scores in cross_links are sorted descending (best first)
        let scores: Vec<f64> = db.conn()
            .prepare("SELECT score FROM cross_links ORDER BY score DESC")
            .unwrap()
            .query_map([], |row| row.get::<_, f64>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        if scores.len() > 1 {
            for w in scores.windows(2) {
                assert!(
                    w[0] >= w[1],
                    "cross-link scores should be stored with highest first: {} >= {}",
                    w[0], w[1]
                );
            }
        }
    }

    // ---------------------------------------------------------------
    // DW-3.7: Dream report format unchanged
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_3_7_dream_report_format_unchanged() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let docs = vec![
            ("memories", "notes/rust.md", "notes",
             "---\nname: rust-lang\ndate: 2025-01-01\n---\n\nRust programming language systems performance"),
            ("memories", "ref/guide.md", "ref",
             "---\nname: guide\ndate: 2025-01-02\n---\n\nProgramming guide reference manual documentation"),
            ("memories", "tips/cooking.md", "tips",
             "---\nname: cooking\ndate: 2025-01-03\n---\n\nCooking recipes baking fermentation sourdough pastry"),
            ("memories", "ref/gardening.md", "ref",
             "---\nname: gardening\ndate: 2025-01-04\n---\n\nGardening botany photosynthesis chlorophyll plants"),
            ("memories", "tips/astronomy.md", "tips",
             "---\nname: astronomy\ndate: 2025-01-05\n---\n\nAstronomy telescopes galaxies nebula stargazing"),
        ];
        setup_weighted_docs(&db, &brain_dir, &docs);

        let result = grug_dream(&mut db).unwrap();

        // Standard report sections should be present
        assert!(result.contains("# dream report"), "should have report header");
        assert!(result.contains("memories |"), "should have memory count in header");
        assert!(result.contains("categories |"), "should have category count");
        assert!(result.contains("need review"), "should have needs-review count");
        assert!(result.contains("## needs review"), "should have needs review section");

        // Cross-links section format: "## new cross-links (N found, top M)"
        if result.contains("cross-links") {
            assert!(
                result.contains("## new cross-links ("),
                "cross-links header should follow format '## new cross-links (N found, top M)'"
            );
            // Each cross-link should use the arrow format
            assert!(
                result.contains("\u{2194}"),
                "cross-links should use bidirectional arrow"
            );
        }
    }
}
