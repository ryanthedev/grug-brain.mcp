use super::GrugDb;
use crate::types::RecallRow;
use std::fs;

/// Read and browse brains. Complex backwards-compat logic matching JS.
/// No args = list all brains. Brain only = list categories.
/// Brain + category = list files. Brain + category + path = read file.
/// Omitting brain searches primary brain first.
pub fn grug_read(
    db: &mut GrugDb,
    brain_name: Option<&str>,
    category: Option<&str>,
    path_name: Option<&str>,
) -> Result<String, String> {
    db.maybe_reload_config();

    // Case 1: No args -> list all brains
    if brain_name.is_none() && category.is_none() && path_name.is_none() {
        return list_all_brains(db);
    }

    // Case 2: Category only (no brain, no path) -> backwards-compat search
    if brain_name.is_none() && category.is_some() && path_name.is_none() {
        return read_category_compat(db, category.unwrap());
    }

    // Case 3: Path only (no brain, no category) -> try primary brain
    if brain_name.is_none() && category.is_none() && path_name.is_some() {
        return read_path_compat(db, path_name.unwrap());
    }

    let brain = db.resolve_brain(brain_name)?.clone();

    // Case 4: Brain only -> list categories
    if category.is_none() && path_name.is_none() {
        return list_categories(db, &brain.name);
    }

    // Case 5: Brain + category -> list files
    if category.is_some() && path_name.is_none() {
        return list_category_files(db, &brain.name, category.unwrap());
    }

    // Case 6: Brain + category + path -> read file
    let cat = category.unwrap_or_else(|| {
        let name = path_name.unwrap();
        name.split('/').next().unwrap_or(name)
    });
    let raw_name = path_name.unwrap();
    let file = if raw_name.contains('/') {
        raw_name.split('/').last().unwrap_or(raw_name)
    } else {
        raw_name
    };
    let t = if file.ends_with(".md") {
        file.to_string()
    } else {
        format!("{file}.md")
    };

    // Flat brains: files live directly in brain.dir
    let file_path = if brain.flat {
        brain.dir.join(&t)
    } else {
        brain.dir.join(cat).join(&t)
    };

    if !file_path.exists() {
        return Ok(format!("not found: {}/{}/{}", brain.name, cat, file));
    }

    let content = fs::read_to_string(&file_path)
        .map_err(|_| format!("could not read: {}/{}/{}", brain.name, cat, file))?;

    // Get cross-links for this file
    let rel_path = format!("{cat}/{t}");
    let links = get_cross_links(db, &brain.name, &rel_path);

    let mut text = content;
    if !links.is_empty() {
        let link_lines: Vec<String> = links
            .iter()
            .map(|l| format!("- {}", l))
            .collect();
        text.push_str(&format!(
            "\n\n---\n## linked memories\n\n{}",
            link_lines.join("\n")
        ));
    }

    Ok(text)
}

fn list_all_brains(db: &GrugDb) -> Result<String, String> {
    let brains = &db.config().brains;
    if brains.is_empty() {
        return Ok("no brains configured".to_string());
    }

    let mut lines = Vec::new();
    for b in brains {
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE brain = ?1",
                [&b.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let mut flags = Vec::new();
        if b.primary {
            flags.push("primary".to_string());
        }
        if b.writable {
            flags.push("writable".to_string());
        } else {
            flags.push("read-only".to_string());
        }
        if b.git.is_some() {
            flags.push("git-synced".to_string());
        }

        lines.push(format!("  {}  ({} files, {})", b.name, count, flags.join(", ")));
    }

    Ok(format!("{} brains\n\n{}", brains.len(), lines.join("\n")))
}

fn read_category_compat(db: &GrugDb, category: &str) -> Result<String, String> {
    let primary = db.config().primary_brain().clone();

    // Search primary brain first
    let primary_rows = recall_by_category(db, &primary.name, category);
    let (target_brain, rows) = if !primary_rows.is_empty() {
        (primary.name.clone(), primary_rows)
    } else {
        // Fall back to any brain that has this category
        let mut found = None;
        for b in &db.config().brains {
            if b.primary {
                continue;
            }
            let rows = recall_by_category(db, &b.name, category);
            if !rows.is_empty() {
                found = Some((b.name.clone(), rows));
                break;
            }
        }
        match found {
            Some(f) => f,
            None => return Ok(format!("no files in \"{category}\"")),
        }
    };

    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let date = if r.date.is_empty() {
                String::new()
            } else {
                format!(" ({})", r.date)
            };
            format!("- {}{}: {}", r.name, date, r.description)
        })
        .collect();

    Ok(format!(
        "# {} [{}] ({} files)\n\n{}",
        category,
        target_brain,
        rows.len(),
        lines.join("\n")
    ))
}

fn read_path_compat(db: &GrugDb, name: &str) -> Result<String, String> {
    let primary = db.config().primary_brain().clone();
    let cat = name.split('/').next().unwrap_or(name);
    let file = if name.contains('/') {
        name.split('/').last().unwrap_or(name)
    } else {
        name
    };
    let t = if file.ends_with(".md") {
        file.to_string()
    } else {
        format!("{file}.md")
    };

    let file_path = primary.dir.join(cat).join(&t);
    if !file_path.exists() {
        return Ok(format!("not found: {name}"));
    }

    fs::read_to_string(&file_path).map_err(|_| format!("could not read: {name}"))
}

fn list_categories(db: &GrugDb, brain_name: &str) -> Result<String, String> {
    let mut stmt = db
        .conn()
        .prepare("SELECT category, COUNT(*) as count FROM brain_fts WHERE brain = ?1 GROUP BY category ORDER BY category")
        .map_err(|e| format!("prepare: {e}"))?;

    let rows: Vec<(String, i32)> = stmt
        .query_map([brain_name], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("query: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!("no categories in brain \"{brain_name}\""));
    }

    let lines: Vec<String> = rows
        .iter()
        .map(|(cat, count)| format!("  {}  ({} files)", cat, count))
        .collect();

    Ok(format!(
        "{} categories in \"{}\"\n\n{}",
        rows.len(),
        brain_name,
        lines.join("\n")
    ))
}

fn list_category_files(db: &GrugDb, brain_name: &str, category: &str) -> Result<String, String> {
    let rows = recall_by_category(db, brain_name, category);
    if rows.is_empty() {
        return Ok(format!("no files in \"{brain_name}/{category}\""));
    }

    let lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let date = if r.date.is_empty() {
                String::new()
            } else {
                format!(" ({})", r.date)
            };
            format!("- {}{}: {}", r.name, date, r.description)
        })
        .collect();

    Ok(format!(
        "# {} [{}] ({} files)\n\n{}",
        category,
        brain_name,
        rows.len(),
        lines.join("\n")
    ))
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

fn get_cross_links(db: &GrugDb, brain_name: &str, rel_path: &str) -> Vec<String> {
    let mut stmt = match db.conn().prepare(
        "SELECT brain_a, path_a, brain_b, path_b, score
         FROM cross_links
         WHERE (brain_a = ?1 AND path_a = ?2) OR (brain_b = ?1 AND path_b = ?2)
         ORDER BY score
         LIMIT 10",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let links: Vec<(String, String, String, String)> = stmt
        .query_map(
            rusqlite::params![brain_name, rel_path],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();

    let mut result = Vec::new();
    for (brain_a, path_a, brain_b, path_b) in links {
        let is_a = path_a == rel_path && brain_a == brain_name;
        let (other_brain, other_path) = if is_a {
            (brain_b, path_b)
        } else {
            (brain_a, path_a)
        };

        // Look up name and category for the linked memory
        let meta = db.conn().query_row(
            "SELECT name, category FROM brain_fts WHERE brain = ?1 AND path = ?2 LIMIT 1",
            rusqlite::params![other_brain, other_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );

        match meta {
            Ok((name, category)) => result.push(format!("{name} [{category}]")),
            Err(_) => result.push(other_path),
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi};

    #[test]
    fn test_read_no_args_lists_brains() {
        let (mut db, _tmp) = test_db();
        let result = grug_read(&mut db, None, None, None).unwrap();
        assert!(result.contains("1 brains"));
        assert!(result.contains("memories"));
        assert!(result.contains("primary"));
    }

    #[test]
    fn test_read_brain_only_lists_categories() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(&brain_dir, "notes/a.md", "---\nname: a\n---\n\nBody");
        index_file(db.conn(), "memories", "notes/a.md", &f, "notes").unwrap();

        let result = grug_read(&mut db, Some("memories"), None, None).unwrap();
        assert!(result.contains("1 categories"));
        assert!(result.contains("notes"));
    }

    #[test]
    fn test_read_brain_category_lists_files() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/test.md",
            "---\nname: test\ndate: 2025-01-01\n---\n\nBody",
        );
        index_file(db.conn(), "memories", "notes/test.md", &f, "notes").unwrap();

        let result = grug_read(&mut db, Some("memories"), Some("notes"), None).unwrap();
        assert!(result.contains("# notes [memories]"));
        assert!(result.contains("test"));
    }

    #[test]
    fn test_read_full_path_reads_file() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/hello.md", "---\nname: hello\n---\n\nHello world");

        let result =
            grug_read(&mut db, Some("memories"), Some("notes"), Some("hello")).unwrap();
        assert!(result.contains("Hello world"));
    }

    #[test]
    fn test_read_category_compat_primary() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\ndate: 2025-01-01\n---\n\nBody A",
        );
        index_file(db.conn(), "memories", "notes/a.md", &f, "notes").unwrap();

        let result = grug_read(&mut db, None, Some("notes"), None).unwrap();
        assert!(result.contains("# notes [memories]"));
        assert!(result.contains("1 files"));
    }

    #[test]
    fn test_read_category_compat_fallback() {
        let (mut db, tmp) = test_db_multi();
        let docs_dir = tmp.path().join("docs");
        let f = create_brain_file(&docs_dir, "api.md", "---\nname: api\n---\n\nAPI docs");
        // For flat brain, category = brain name
        index_file(db.conn(), "docs", "api.md", &f, "docs").unwrap();

        // "docs" category not in primary -> falls back to docs brain
        let result = grug_read(&mut db, None, Some("docs"), None).unwrap();
        assert!(result.contains("[docs]"));
    }

    #[test]
    fn test_read_path_compat() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/test.md", "The file content");

        let result = grug_read(&mut db, None, None, Some("notes/test")).unwrap();
        assert!(result.contains("The file content"));
    }

    #[test]
    fn test_read_not_found() {
        let (mut db, _tmp) = test_db();
        let result = grug_read(
            &mut db,
            Some("memories"),
            Some("notes"),
            Some("nonexistent"),
        )
        .unwrap();
        assert!(result.contains("not found"));
    }

    #[test]
    fn test_read_with_cross_links() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f1 = create_brain_file(&brain_dir, "notes/a.md", "---\nname: a\n---\n\nBody A");
        let f2 = create_brain_file(&brain_dir, "ref/b.md", "---\nname: b\n---\n\nBody B");
        index_file(db.conn(), "memories", "notes/a.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/b.md", &f2, "ref").unwrap();

        // Insert a cross-link
        db.conn()
            .execute(
                "INSERT INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES ('memories', 'notes/a.md', 'memories', 'ref/b.md', -1.0, '2025-01-01')",
                [],
            )
            .unwrap();

        let result =
            grug_read(&mut db, Some("memories"), Some("notes"), Some("a")).unwrap();
        assert!(result.contains("linked memories"));
        assert!(result.contains("b [ref]"));
    }
}
