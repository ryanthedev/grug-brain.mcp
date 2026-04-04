use super::GrugDb;
use crate::types::RecallRow;
use std::fs;

/// Get up to speed. Shows 2 most recent per category,
/// writes full listing to recall.md in the primary brain's directory.
///
/// When no brain is specified, shows ALL brains (not just primary).
/// When a category is specified without a brain, searches all brains for that category.
pub fn grug_recall(
    db: &mut GrugDb,
    category: Option<&str>,
    brain_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();

    // Collect rows: if brain specified, use that brain. Otherwise, all brains.
    let rows = match brain_name {
        Some(_) => {
            let brain = db.resolve_brain(brain_name)?.clone();
            if let Some(cat) = category {
                recall_by_category(db, &brain.name, cat)
            } else {
                recall_all(db, &brain.name)
            }
        }
        None => {
            // No brain specified: search ALL brains
            let brain_names: Vec<String> = db.config().brains.iter().map(|b| b.name.clone()).collect();
            let mut all_rows = Vec::new();
            for name in &brain_names {
                if let Some(cat) = category {
                    all_rows.extend(recall_by_category(db, name, cat));
                } else {
                    all_rows.extend(recall_all(db, name));
                }
            }
            all_rows
        }
    };

    if rows.is_empty() {
        let cat_msg = category
            .map(|c| format!(" in \"{c}\""))
            .unwrap_or_default();
        let brain_msg = brain_name
            .map(|b| format!(" in brain \"{b}\""))
            .unwrap_or_default();
        return Ok(format!("no memories found{}{}", cat_msg, brain_msg));
    }

    // Group rows by category (preserving insertion order)
    let mut groups: Vec<(String, Vec<&RecallRow>)> = Vec::new();
    for row in &rows {
        if let Some((_cat, entries)) = groups.last_mut().filter(|(c, _)| c == &row.category) {
            entries.push(row);
        } else {
            groups.push((row.category.clone(), vec![row]));
        }
    }

    let primary_name = db.config().primary.clone();

    // Write full listing to recall.md in primary brain directory
    let primary_dir = db.config().primary_brain().dir.clone();
    let mut full_lines = Vec::new();
    for (cat, entries) in &groups {
        full_lines.push(format!("# {cat}\n"));
        for e in entries {
            let date = if e.date.is_empty() {
                String::new()
            } else {
                format!(" ({})", e.date)
            };
            full_lines.push(format!("- [{}]({}){}: {}", e.name, e.path, date, e.description));
        }
        full_lines.push(String::new());
    }
    let out_path = primary_dir.join("recall.md");
    fs::write(&out_path, full_lines.join("\n"))
        .map_err(|e| format!("failed to write recall.md: {e}"))?;

    // Build preview: 2 most recent per category, with brain tags
    let mut preview = Vec::new();
    for (cat, entries) in &groups {
        preview.push(format!("# {cat}"));
        for e in entries.iter().take(2) {
            let date = if e.date.is_empty() {
                String::new()
            } else {
                format!(" ({})", e.date)
            };
            let brain_tag = if e.brain != primary_name {
                format!(" [{}]", e.brain)
            } else {
                String::new()
            };
            preview.push(format!("- {}{}{}: {}", e.name, brain_tag, date, e.description));
        }
        if entries.len() > 2 {
            preview.push(format!("  \u{2026} and {} more", entries.len() - 2));
        }
    }

    Ok(format!("{}\n\n{}", out_path.display(), preview.join("\n")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db};

    #[test]
    fn test_recall_empty() {
        let (mut db, _tmp) = test_db();
        let result = grug_recall(&mut db, None, None).unwrap();
        assert!(result.contains("no memories found"));
    }

    #[test]
    fn test_recall_with_category_filter() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\ndate: 2025-01-01\n---\n\nBody A",
        );
        index_file(db.conn(), "memories", "notes/a.md", &f, "notes").unwrap();

        let result = grug_recall(&mut db, Some("notes"), None).unwrap();
        assert!(result.contains("# notes"));
        assert!(result.contains("a"));
    }

    #[test]
    fn test_recall_preview_limits_to_2() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        for i in 1..=5 {
            let f = create_brain_file(
                &brain_dir,
                &format!("notes/{i}.md"),
                &format!("---\nname: item-{i}\ndate: 2025-01-0{i}\n---\n\nBody {i}"),
            );
            index_file(
                db.conn(),
                "memories",
                &format!("notes/{i}.md"),
                &f,
                "notes",
            )
            .unwrap();
        }

        let result = grug_recall(&mut db, None, None).unwrap();
        assert!(result.contains("\u{2026} and 3 more"));
    }

    #[test]
    fn test_recall_writes_recall_md() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/test.md",
            "---\nname: test\ndate: 2025-01-01\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/test.md", &f, "notes").unwrap();

        grug_recall(&mut db, None, None).unwrap();

        let recall_path = tmp.path().join("memories/recall.md");
        assert!(recall_path.exists());
        let content = fs::read_to_string(recall_path).unwrap();
        assert!(content.contains("# notes"));
        assert!(content.contains("[test](notes/test.md)"));
    }

    #[test]
    fn test_recall_multiple_categories() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let f1 = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\ndate: 2025-01-01\n---\n\nBody",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "ref/b.md",
            "---\nname: b\ndate: 2025-02-01\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/a.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/b.md", &f2, "ref").unwrap();

        let result = grug_recall(&mut db, None, None).unwrap();
        assert!(result.contains("# notes"));
        assert!(result.contains("# ref"));
    }
}
