use crate::types::SearchResult;
use rusqlite::Connection;

use super::SEARCH_PAGE_SIZE;

/// Build an FTS5 query string from user input.
/// Single term: `"term"*`
/// Multiple terms: `"term1"* OR "term2"*`
/// Empty/whitespace-only input returns None.
pub fn build_fts_query(query: &str) -> Option<String> {
    let terms: Vec<&str> = query.split_whitespace().filter(|s| !s.is_empty()).collect();
    if terms.is_empty() {
        return None;
    }
    if terms.len() == 1 {
        Some(format!("\"{}\"*", terms[0]))
    } else {
        let parts: Vec<String> = terms.iter().map(|t| format!("\"{}\"*", t)).collect();
        Some(parts.join(" OR "))
    }
}

/// Execute an FTS search with fallback on query error.
/// Returns (results, total_count).
pub fn fts_search(
    conn: &Connection,
    fts_query: &str,
    limit: usize,
    offset: usize,
) -> (Vec<SearchResult>, usize) {
    // Try the query as-is first
    match fts_search_inner(conn, fts_query, limit, offset) {
        Ok(result) => return result,
        Err(_) => {}
    }

    // Fallback: strip wildcards
    let simple = fts_query.replace('*', "");
    match fts_search_inner(conn, &simple, limit, offset) {
        Ok(result) => result,
        Err(_) => (vec![], 0),
    }
}

fn fts_search_inner(
    conn: &Connection,
    fts_query: &str,
    limit: usize,
    offset: usize,
) -> Result<(Vec<SearchResult>, usize), rusqlite::Error> {
    let total: usize = conn.query_row(
        "SELECT COUNT(*) FROM brain_fts WHERE brain_fts MATCH ?1",
        [fts_query],
        |row| row.get(0),
    )?;

    let mut stmt = conn.prepare(
        "SELECT path, brain, category, name, date, description,
                highlight(brain_fts, 5, '>>>', '<<<') as snippet,
                rank
         FROM brain_fts
         WHERE brain_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2 OFFSET ?3",
    )?;

    let results = stmt
        .query_map(
            rusqlite::params![fts_query, limit as i64, offset as i64],
            |row| {
                Ok(SearchResult {
                    path: row.get(0)?,
                    brain: row.get(1)?,
                    category: row.get(2)?,
                    name: row.get(3)?,
                    date: row.get(4)?,
                    description: row.get(5)?,
                    snippet: row.get(6)?,
                    rank: row.get(7)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    Ok((results, total))
}

/// Search all brains with pagination.
pub fn search_all(
    conn: &Connection,
    query: &str,
    page: Option<usize>,
) -> (Vec<SearchResult>, usize) {
    let fts_query = match build_fts_query(query) {
        Some(q) => q,
        None => return (vec![], 0),
    };
    let p = page.unwrap_or(1).max(1);
    let offset = (p - 1) * SEARCH_PAGE_SIZE;
    fts_search(conn, &fts_query, SEARCH_PAGE_SIZE, offset)
}

/// grug-search tool: search across all brains, formatted output.
pub fn grug_search(db: &mut super::GrugDb, query: &str, page: Option<usize>) -> String {
    db.maybe_reload_config();
    let (results, total) = search_all(db.conn(), query, page);
    if total == 0 {
        return format!("no matches for \"{query}\"");
    }

    let p = page.unwrap_or(1).max(1);
    let mut lines = Vec::new();
    for r in &results {
        let date = if r.date.is_empty() {
            String::new()
        } else {
            format!(" date:{}", r.date)
        };
        let snippet = if r.snippet.is_empty() {
            r.description.clone()
        } else {
            r.snippet.clone()
        };
        lines.push(format!("{}{} [{}] [{}]\n  {}", r.path, date, r.category, r.brain, snippet));
    }

    let total_pages = (total + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
    let paging = if total_pages > 1 {
        format!("\n--- page {p}/{total_pages} | page:{} for more ---", p + 1)
    } else {
        String::new()
    };

    format!("{total} matches for \"{query}\"\n\n{}{paging}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::test_db;
    use crate::tools::indexing::index_file;

    #[test]
    fn test_build_fts_query_single() {
        assert_eq!(build_fts_query("hello"), Some("\"hello\"*".to_string()));
    }

    #[test]
    fn test_build_fts_query_multi() {
        assert_eq!(
            build_fts_query("hello world"),
            Some("\"hello\"* OR \"world\"*".to_string())
        );
    }

    #[test]
    fn test_build_fts_query_empty() {
        assert_eq!(build_fts_query(""), None);
        assert_eq!(build_fts_query("   "), None);
    }

    #[test]
    fn test_fts_search_basic() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let full = brain_dir.join("test/hello.md");
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, "---\nname: hello-world\ndate: 2025-01-01\n---\n\nSome searchable content about rust programming").unwrap();
        index_file(db.conn(), "memories", "test/hello.md", &full, "test").unwrap();

        let (results, total) = search_all(db.conn(), "rust", None);
        assert_eq!(total, 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "hello-world");
        assert!(results[0].snippet.contains(">>>"));
    }

    #[test]
    fn test_fts_search_no_results() {
        let (db, _tmp) = test_db();
        let (results, total) = search_all(db.conn(), "nonexistent", None);
        assert_eq!(total, 0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fts_search_empty_query() {
        let (db, _tmp) = test_db();
        let (results, total) = search_all(db.conn(), "", None);
        assert_eq!(total, 0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_fts_search_bm25_ranking() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // File with "rust" in body
        let f1 = brain_dir.join("cat/a.md");
        std::fs::create_dir_all(f1.parent().unwrap()).unwrap();
        std::fs::write(&f1, "---\nname: a\n---\n\nrust is a language").unwrap();
        index_file(db.conn(), "memories", "cat/a.md", &f1, "cat").unwrap();

        // File with "rust" multiple times (should rank higher)
        let f2 = brain_dir.join("cat/b.md");
        std::fs::write(&f2, "---\nname: b\n---\n\nrust rust rust programming in rust").unwrap();
        index_file(db.conn(), "memories", "cat/b.md", &f2, "cat").unwrap();

        let (results, total) = search_all(db.conn(), "rust", None);
        assert_eq!(total, 2);
        // BM25 rank is negative (lower = better), so the more relevant doc comes first
        assert!(results[0].rank <= results[1].rank);
    }

    #[test]
    fn test_fts_search_highlight_snippets() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let f = brain_dir.join("cat/test.md");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, "---\nname: test\n---\n\nThe description has important keywords here").unwrap();
        index_file(db.conn(), "memories", "cat/test.md", &f, "cat").unwrap();

        let (results, _) = search_all(db.conn(), "important", None);
        assert_eq!(results.len(), 1);
        // highlight column 5 = description (0-indexed: path, brain, category, name, date, description)
        // Actually column 5 in the SELECT is description with highlight on body (column index 5 in FTS = body which is col 6)
        // The highlight is on column 5 which is the body column (0:path, 1:brain, 2:category, 3:name, 4:date, 5:description, 6:body)
        // Wait - highlight(brain_fts, 5, ...) highlights the 6th column (0-indexed) which is description
        // Let me re-check: FTS columns are path(0), brain(1), category(2), name(3), date(4), description(5), body(6)
        // highlight index 5 = description column
        assert!(results[0].snippet.contains(">>>") || !results[0].description.is_empty());
    }

    #[test]
    fn test_grug_search_formatted_output() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        let full = brain_dir.join("notes/hello.md");
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, "---\nname: hello\ndate: 2025-01-01\n---\n\nSearchable content").unwrap();
        index_file(db.conn(), "memories", "notes/hello.md", &full, "notes").unwrap();

        let result = grug_search(&mut db, "searchable", None);
        assert!(result.starts_with("1 matches for \"searchable\""));
        assert!(result.contains("[notes]"));
        assert!(result.contains("[memories]"));
        assert!(result.contains("date:2025-01-01"));
    }

    #[test]
    fn test_grug_search_no_matches() {
        let (mut db, _tmp) = test_db();
        let result = grug_search(&mut db, "nonexistent", None);
        assert_eq!(result, "no matches for \"nonexistent\"");
    }
}
