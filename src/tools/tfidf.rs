use rusqlite::Connection;

/// Maximum number of top-weighted terms to store per document.
pub const TOP_N: usize = 50;

/// Maximum document frequency ratio. Terms appearing in more than this
/// fraction of total documents are excluded (stopword-like filtering).
pub const MAX_DF_RATIO: f64 = 0.5;

/// Minimum term length. Terms shorter than this are excluded.
pub const MIN_TERM_LEN: usize = 3;

/// Compute TF-IDF weights for a document and store the top-N in term_weights.
/// Also computes and stores the L2 norm in doc_norms.
///
/// Must be called AFTER the document's FTS5 row has been inserted, so the
/// fts5vocab tables reflect the current state.
pub fn compute_and_store_weights(
    conn: &Connection,
    brain: &str,
    path: &str,
) -> Result<(), String> {
    // Ensure vocab virtual tables exist
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS brain_vocab_row USING fts5vocab('brain_fts', 'row');
         CREATE VIRTUAL TABLE IF NOT EXISTS brain_vocab_inst USING fts5vocab('brain_fts', 'instance');",
    )
    .map_err(|e| format!("create vocab tables: {e}"))?;

    // Get total document count
    let total_docs: f64 = conn
        .query_row("SELECT COUNT(*) FROM brain_fts", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| format!("count docs: {e}"))? as f64;

    if total_docs == 0.0 {
        return Ok(());
    }

    // Get rowid for this document
    let rowid: i64 = match conn.query_row(
        "SELECT rowid FROM brain_fts WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain, path],
        |row| row.get(0),
    ) {
        Ok(r) => r,
        Err(_) => return Ok(()), // Document not in FTS, nothing to do
    };

    // Get per-document term frequencies and document frequencies.
    // TF-IDF computed in Rust since bundled SQLite lacks math functions.
    // TF = 1 + ln(count_in_doc)
    // IDF = ln((N - df + 0.5) / (df + 0.5) + 1)
    let mut stmt = conn
        .prepare(
            "WITH doc_terms AS (
                SELECT term, COUNT(*) as tf
                FROM brain_vocab_inst
                WHERE doc = ?1
                GROUP BY term
            )
            SELECT dt.term, dt.tf, vr.doc
            FROM doc_terms dt
            JOIN brain_vocab_row vr ON dt.term = vr.term
            WHERE length(dt.term) >= ?2",
        )
        .map_err(|e| format!("prepare tfidf query: {e}"))?;

    let raw_terms: Vec<(String, i64, i64)> = stmt
        .query_map(
            rusqlite::params![rowid, MIN_TERM_LEN as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)),
        )
        .map_err(|e| format!("query tfidf: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    // Compute TF-IDF weights in Rust, applying DF threshold filter
    let mut terms: Vec<(String, f64)> = raw_terms
        .into_iter()
        .filter(|(_, _, df)| (*df as f64) / total_docs <= MAX_DF_RATIO)
        .map(|(term, tf, df)| {
            let tf_weight = 1.0 + (tf as f64).ln();
            let idf = ((total_docs - df as f64 + 0.5) / (df as f64 + 0.5) + 1.0).ln();
            (term, tf_weight * idf)
        })
        .collect();

    // Sort by weight descending, take top N
    terms.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    terms.truncate(TOP_N);

    // Delete old weights for this document
    conn.execute(
        "DELETE FROM term_weights WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain, path],
    )
    .map_err(|e| format!("delete old weights: {e}"))?;

    // Insert new weights
    let mut insert_stmt = conn
        .prepare(
            "INSERT INTO term_weights (brain, path, term, weight) VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(|e| format!("prepare insert weights: {e}"))?;

    let mut norm_sq = 0.0f64;
    for (term, weight) in &terms {
        insert_stmt
            .execute(rusqlite::params![brain, path, term, weight])
            .map_err(|e| format!("insert weight: {e}"))?;
        norm_sq += weight * weight;
    }

    // Compute and store L2 norm
    let norm = norm_sq.sqrt();
    conn.execute(
        "INSERT OR REPLACE INTO doc_norms (brain, path, norm) VALUES (?1, ?2, ?3)",
        rusqlite::params![brain, path, norm],
    )
    .map_err(|e| format!("upsert norm: {e}"))?;

    Ok(())
}

/// Remove term weights and doc norm for a document.
pub fn remove_weights(
    conn: &Connection,
    brain: &str,
    path: &str,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM term_weights WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain, path],
    )
    .map_err(|e| format!("delete weights: {e}"))?;

    conn.execute(
        "DELETE FROM doc_norms WHERE brain = ?1 AND path = ?2",
        rusqlite::params![brain, path],
    )
    .map_err(|e| format!("delete norm: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db};

    #[test]
    fn test_dw_1_3_index_computes_tfidf() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create multiple docs so IDF is meaningful
        let f1 = create_brain_file(
            &brain_dir,
            "notes/rust-guide.md",
            "---\nname: rust-guide\n---\n\nRust programming language for systems. Rust is fast and safe.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/python-guide.md",
            "---\nname: python-guide\n---\n\nPython scripting language for web development.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/go-guide.md",
            "---\nname: go-guide\n---\n\nGo programming language for services and concurrency.",
        );

        index_file(db.conn(), "memories", "notes/rust-guide.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/python-guide.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/go-guide.md", &f3, "ref").unwrap();

        // Compute TF-IDF for the rust doc
        compute_and_store_weights(db.conn(), "memories", "notes/rust-guide.md").unwrap();

        // Should have some term weights stored
        let count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/rust-guide.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(count > 0, "should have stored term weights");

        // Rust-specific terms should have high weights
        let rust_weight: f64 = db.conn()
            .query_row(
                "SELECT weight FROM term_weights WHERE brain = 'memories' AND path = 'notes/rust-guide.md' AND term = 'rust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(rust_weight > 0.0, "rust term should have positive weight");
    }

    #[test]
    fn test_dw_1_3_top_n_limit() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create a doc with lots of unique terms
        let long_body = (0..100)
            .map(|i| format!("uniqueterm{i:03}"))
            .collect::<Vec<_>>()
            .join(" ");
        let content = format!("---\nname: many-terms\n---\n\n{long_body}");
        let f = create_brain_file(&brain_dir, "notes/many.md", &content);

        index_file(db.conn(), "memories", "notes/many.md", &f, "notes").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/many.md").unwrap();

        let count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/many.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            count <= TOP_N as i32,
            "should store at most TOP_N ({TOP_N}) terms, got {count}"
        );
    }

    #[test]
    fn test_dw_1_4_remove_cleans_weights_and_norms() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Need multiple docs so terms have DF ratio < 0.5
        let f = create_brain_file(
            &brain_dir,
            "notes/doomed.md",
            "---\nname: doomed\n---\n\nThis document has unique specialized content about algorithms.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/other.md",
            "---\nname: other\n---\n\nCompletely different topic about gardening and botany.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/third.md",
            "---\nname: third\n---\n\nAnother unrelated document about cooking recipes.",
        );
        index_file(db.conn(), "memories", "notes/doomed.md", &f, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/other.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/third.md", &f3, "ref").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/doomed.md").unwrap();

        // Verify data exists
        let tw_count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(tw_count > 0, "should have term weights before removal");

        let dn_count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM doc_norms WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dn_count, 1, "should have doc norm before removal");

        // Remove
        remove_weights(db.conn(), "memories", "notes/doomed.md").unwrap();

        // Verify cleaned
        let tw_after: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tw_after, 0, "term weights should be gone after removal");

        let dn_after: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM doc_norms WHERE brain = 'memories' AND path = 'notes/doomed.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dn_after, 0, "doc norm should be gone after removal");
    }

    #[test]
    fn test_dw_1_6_idf_threshold_filtering() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create 4 docs where "language" appears in all 4 (DF ratio = 1.0 > 0.5)
        // and "rust" appears in only 1 (DF ratio = 0.25 < 0.5)
        let f1 = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\n---\n\nRust is a programming language for systems",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/b.md",
            "---\nname: b\n---\n\nPython is a programming language for web",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/c.md",
            "---\nname: c\n---\n\nGo is a programming language for services",
        );
        let f4 = create_brain_file(
            &brain_dir,
            "ref/d.md",
            "---\nname: d\n---\n\nJava is a programming language for enterprise",
        );

        index_file(db.conn(), "memories", "notes/a.md", &f1, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/b.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/c.md", &f3, "ref").unwrap();
        index_file(db.conn(), "memories", "ref/d.md", &f4, "ref").unwrap();

        compute_and_store_weights(db.conn(), "memories", "notes/a.md").unwrap();

        // "languag" (stemmed) appears in 4/4 docs = 100% > 50%, should be filtered
        let lang_count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/a.md' AND term = 'languag'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(lang_count, 0, "'languag' appears in >50% of docs, should be filtered");

        // "rust" appears in 1/4 docs = 25% < 50%, should be kept
        let rust_count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/a.md' AND term = 'rust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rust_count, 1, "'rust' appears in 25% of docs, should be kept");
    }

    #[test]
    fn test_dw_1_7_doc_norms_populated() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Need multiple docs so terms have DF ratio < 0.5
        let f = create_brain_file(
            &brain_dir,
            "notes/test.md",
            "---\nname: test\n---\n\nSome unique content with distinctive words about specialized topics.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/other.md",
            "---\nname: other\n---\n\nCompletely different topic about gardening and botany.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/third.md",
            "---\nname: third\n---\n\nAnother unrelated document about cooking recipes.",
        );
        index_file(db.conn(), "memories", "notes/test.md", &f, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/other.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/third.md", &f3, "ref").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/test.md").unwrap();

        // Verify doc_norms has an entry
        let norm: f64 = db.conn()
            .query_row(
                "SELECT norm FROM doc_norms WHERE brain = 'memories' AND path = 'notes/test.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(norm > 0.0, "L2 norm should be positive, got {norm}");

        // Verify norm is consistent with stored weights
        // L2 norm = sqrt(sum(weight^2))
        let sum_sq: f64 = db.conn()
            .query_row(
                "SELECT SUM(weight * weight) FROM term_weights WHERE brain = 'memories' AND path = 'notes/test.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let expected_norm = sum_sq.sqrt();
        assert!(
            (norm - expected_norm).abs() < 0.0001,
            "norm ({norm}) should equal sqrt(sum(w^2)) ({expected_norm})"
        );
    }

    #[test]
    fn test_dw_1_8_empty_document_no_weights() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create a doc with no meaningful content (only short/common words)
        let f = create_brain_file(
            &brain_dir,
            "notes/empty.md",
            "---\nname: empty\n---\n\n",
        );
        index_file(db.conn(), "memories", "notes/empty.md", &f, "notes").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/empty.md").unwrap();

        // Should have no term weights (or very few from just the name/category)
        let count: i32 = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/empty.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // The name "empty" is 5 chars and will be in the FTS index, so count may be small but > 0
        // The key assertion is that it doesn't error and produces reasonable results
        assert!(count <= 5, "empty body doc should have very few terms, got {count}");
    }

    #[test]
    fn test_dw_1_8_reindex_updates_weights() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Need multiple docs so terms have DF ratio < 0.5
        let f = create_brain_file(
            &brain_dir,
            "notes/evolving.md",
            "---\nname: evolving\n---\n\nOriginal content about rust programming systems.",
        );
        let f2 = create_brain_file(
            &brain_dir,
            "notes/other.md",
            "---\nname: other\n---\n\nCompletely different topic about gardening and botany.",
        );
        let f3 = create_brain_file(
            &brain_dir,
            "ref/third.md",
            "---\nname: third\n---\n\nAnother unrelated document about cooking recipes.",
        );
        index_file(db.conn(), "memories", "notes/evolving.md", &f, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/other.md", &f2, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/third.md", &f3, "ref").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/evolving.md").unwrap();

        // Check initial weights
        let has_rust: bool = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/evolving.md' AND term = 'rust'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap() > 0;
        assert!(has_rust, "should have 'rust' term initially");

        // Update the file content (replace rust with python)
        std::fs::write(
            &f,
            "---\nname: evolving\n---\n\nUpdated content about python scripting web development.",
        )
        .unwrap();

        // Re-index
        index_file(db.conn(), "memories", "notes/evolving.md", &f, "notes").unwrap();
        compute_and_store_weights(db.conn(), "memories", "notes/evolving.md").unwrap();

        // Old term should be gone, new term should be present
        let has_rust_after: bool = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/evolving.md' AND term = 'rust'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap() > 0;
        assert!(!has_rust_after, "'rust' should be gone after re-indexing with new content");

        let has_python: bool = db.conn()
            .query_row(
                "SELECT COUNT(*) FROM term_weights WHERE brain = 'memories' AND path = 'notes/evolving.md' AND term = 'python'",
                [],
                |row| row.get::<_, i32>(0),
            )
            .unwrap() > 0;
        assert!(has_python, "'python' should be present after re-indexing");
    }
}
