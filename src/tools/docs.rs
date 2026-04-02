use super::{GrugDb, BROWSE_PAGE_SIZE};
use crate::helpers::paginate;
use std::fs;

/// [Deprecated: use grug-read] Browse documentation brains (non-primary brains).
pub fn grug_docs(
    db: &mut GrugDb,
    category: Option<&str>,
    path_target: Option<&str>,
    page: Option<usize>,
) -> Result<String, String> {
    db.maybe_reload_config();
    let primary_name = db.config().primary.clone();

    // No args: list categories across all non-primary brains
    if category.is_none() && path_target.is_none() {
        return list_doc_categories(db, &primary_name);
    }

    // Path provided: resolve and read file
    if let Some(target) = path_target {
        return read_doc_file(db, target, &primary_name, page);
    }

    // Category only: list files in first matching non-primary brain
    let cat = category.unwrap();
    list_category_docs(db, cat, &primary_name, page)
}

fn list_doc_categories(db: &GrugDb, primary_name: &str) -> Result<String, String> {
    let mut stmt = db
        .conn()
        .prepare("SELECT brain, category, COUNT(*) as count FROM brain_fts GROUP BY brain, category ORDER BY brain, category")
        .map_err(|e| format!("prepare: {e}"))?;

    let rows: Vec<(String, String, i32)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| format!("query: {e}"))?
        .filter_map(|r| r.ok())
        .filter(|(brain, _, _)| brain != primary_name)
        .collect();

    if rows.is_empty() {
        return Ok("no docs found".to_string());
    }

    let lines: Vec<String> = rows
        .iter()
        .map(|(_, cat, count)| format!("  {cat}  ({count} docs)"))
        .collect();

    Ok(format!("{} doc categories\n\n{}", rows.len(), lines.join("\n")))
}

fn read_doc_file(
    db: &GrugDb,
    target: &str,
    primary_name: &str,
    page: Option<usize>,
) -> Result<String, String> {
    // Try to resolve via brain directories
    let file_path = resolve_doc_path(db, target, primary_name);

    let path = match file_path {
        Some(p) if p.exists() => p,
        _ => {
            // Try as absolute path
            let abs = std::path::PathBuf::from(target);
            if abs.exists() {
                abs
            } else {
                return Ok(format!("file not found: {target}"));
            }
        }
    };

    let content =
        fs::read_to_string(&path).map_err(|_| format!("could not read: {target}"))?;

    Ok(paginate(&content, page.unwrap_or(1)))
}

fn list_category_docs(
    db: &GrugDb,
    category: &str,
    _primary_name: &str,
    page: Option<usize>,
) -> Result<String, String> {
    let non_primary: Vec<_> = db
        .config()
        .brains
        .iter()
        .filter(|b| !b.primary)
        .collect();

    // Find first non-primary brain with this category
    let matching_brain = non_primary.iter().find(|b| {
        db.conn()
            .prepare("SELECT category FROM brain_fts WHERE brain = ?1 GROUP BY category")
            .ok()
            .and_then(|mut s| {
                s.query_map([&b.name], |row| row.get::<_, String>(0))
                    .ok()
                    .map(|rows| rows.filter_map(|r| r.ok()).any(|c| c == category))
            })
            .unwrap_or(false)
    });

    let matching_brain = match matching_brain {
        Some(b) => b,
        None => return Ok(format!("no docs in \"{category}\"")),
    };

    let p = page.unwrap_or(1).max(1);
    let offset = (p - 1) * BROWSE_PAGE_SIZE;

    let total: i32 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM brain_fts WHERE brain = ?1 AND category = ?2",
            rusqlite::params![matching_brain.name, category],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if total == 0 {
        return Ok(format!("no docs in \"{category}\""));
    }

    let mut stmt = db
        .conn()
        .prepare("SELECT path, name, description FROM brain_fts WHERE brain = ?1 AND category = ?2 ORDER BY name LIMIT ?3 OFFSET ?4")
        .map_err(|e| format!("prepare: {e}"))?;

    let rows: Vec<(String, String, String)> = stmt
        .query_map(
            rusqlite::params![matching_brain.name, category, BROWSE_PAGE_SIZE as i64, offset as i64],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("query: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    let lines: Vec<String> = rows
        .iter()
        .map(|(path, name, desc)| format!("- [{name}]({path}): {desc}"))
        .collect();

    let total_pages = (total as usize + BROWSE_PAGE_SIZE - 1) / BROWSE_PAGE_SIZE;
    let paging = if total_pages > 1 {
        format!(
            "\n--- page {p}/{total_pages} ({total} docs) | page:{} for more ---",
            p + 1
        )
    } else {
        String::new()
    };

    Ok(format!(
        "# {category} ({total} docs)\n\n{}{}",
        lines.join("\n"),
        paging
    ))
}

fn resolve_doc_path(
    db: &GrugDb,
    rel_path: &str,
    _primary_name: &str,
) -> Option<std::path::PathBuf> {
    let first_part = rel_path.split('/').next()?;

    // Try category brains: look for a brain with this category
    for brain in &db.config().brains {
        if brain.flat {
            continue;
        }
        let cat_dir = brain.dir.join(first_part);
        if cat_dir.is_dir() {
            let full = brain.dir.join(rel_path);
            if full.exists() {
                return Some(full);
            }
        }
    }

    // Try flat brains: no category prefix
    for brain in &db.config().brains {
        if !brain.flat {
            continue;
        }
        let full = brain.dir.join(rel_path);
        if full.exists() {
            return Some(full);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db_multi};

    #[test]
    fn test_docs_no_args() {
        let (mut db, tmp) = test_db_multi();
        let docs_dir = tmp.path().join("docs");
        let f = create_brain_file(&docs_dir, "guide.md", "---\nname: guide\n---\n\nGuide content");
        index_file(db.conn(), "docs", "guide.md", &f, "docs").unwrap();

        let result = grug_docs(&mut db, None, None, None).unwrap();
        assert!(result.contains("doc categories"));
        assert!(result.contains("docs"));
    }

    #[test]
    fn test_docs_no_docs() {
        let (mut db, _tmp) = test_db_multi();
        let result = grug_docs(&mut db, None, None, None).unwrap();
        assert!(result.contains("no docs found"));
    }

    #[test]
    fn test_docs_category_list() {
        let (mut db, tmp) = test_db_multi();
        let docs_dir = tmp.path().join("docs");
        let f = create_brain_file(&docs_dir, "api.md", "---\nname: api-ref\n---\n\nAPI reference");
        index_file(db.conn(), "docs", "api.md", &f, "docs").unwrap();

        let result = grug_docs(&mut db, Some("docs"), None, None).unwrap();
        assert!(result.contains("# docs"));
        assert!(result.contains("api-ref"));
    }

    #[test]
    fn test_docs_read_file() {
        let (mut db, tmp) = test_db_multi();
        let docs_dir = tmp.path().join("docs");
        create_brain_file(&docs_dir, "guide.md", "The full guide content here");

        let result = grug_docs(&mut db, None, Some("guide.md"), None).unwrap();
        assert!(result.contains("The full guide content here"));
    }

    #[test]
    fn test_docs_category_not_found() {
        let (mut db, _tmp) = test_db_multi();
        let result = grug_docs(&mut db, Some("nonexistent"), None, None).unwrap();
        assert!(result.contains("no docs in"));
    }
}
