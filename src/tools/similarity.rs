use crate::types::SimilarDoc;
use rusqlite::Connection;
use std::collections::HashMap;

/// Find the top-K most similar documents to the given document using
/// cosine similarity on precomputed TF-IDF term weight vectors.
///
/// Uses the `term_weights` inverted index for sublinear candidate retrieval
/// and `doc_norms` for precomputed L2 norms.
pub fn find_similar(
    conn: &Connection,
    brain: &str,
    path: &str,
    limit: usize,
) -> Result<Vec<SimilarDoc>, String> {
    // Step 1: Get the query document's term weights
    let mut stmt = conn
        .prepare("SELECT term, weight FROM term_weights WHERE brain = ?1 AND path = ?2")
        .map_err(|e| format!("prepare query terms: {e}"))?;

    let query_terms: Vec<(String, f64)> = stmt
        .query_map(rusqlite::params![brain, path], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
        })
        .map_err(|e| format!("query terms: {e}"))?
        .filter_map(|r| r.ok())
        .collect();

    if query_terms.is_empty() {
        return Ok(vec![]);
    }

    // Build a lookup map for the query document's terms
    let query_map: HashMap<&str, f64> = query_terms
        .iter()
        .map(|(t, w)| (t.as_str(), *w))
        .collect();

    // Step 2: Get the query document's L2 norm
    let query_norm: f64 = conn
        .query_row(
            "SELECT norm FROM doc_norms WHERE brain = ?1 AND path = ?2",
            rusqlite::params![brain, path],
            |row| row.get(0),
        )
        .map_err(|e| format!("query norm: {e}"))?;

    if query_norm == 0.0 {
        return Ok(vec![]);
    }

    // Step 3: Find all candidate documents sharing any term with the query doc.
    // Uses the idx_term_weights_term index for sublinear retrieval.
    // Build a parameterized IN clause for the query terms.
    let placeholders: Vec<String> = (0..query_terms.len())
        .map(|i| format!("?{}", i + 1))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT brain, path, term, weight FROM term_weights WHERE term IN ({in_clause})"
    );

    let mut candidate_stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("prepare candidates: {e}"))?;

    // Bind term parameters
    let term_params: Vec<&dyn rusqlite::types::ToSql> = query_terms
        .iter()
        .map(|(t, _)| t as &dyn rusqlite::types::ToSql)
        .collect();

    // Accumulate dot products per candidate: (brain, path) -> dot_product
    let mut dot_products: HashMap<(String, String), f64> = HashMap::new();

    let rows = candidate_stmt
        .query_map(term_params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })
        .map_err(|e| format!("query candidates: {e}"))?;

    for row in rows {
        let (cand_brain, cand_path, term, cand_weight) =
            row.map_err(|e| format!("read candidate row: {e}"))?;

        // Exclude self-match
        if cand_brain == brain && cand_path == path {
            continue;
        }

        // Add to dot product: query_weight * candidate_weight
        if let Some(&query_weight) = query_map.get(term.as_str()) {
            *dot_products
                .entry((cand_brain, cand_path))
                .or_insert(0.0) += query_weight * cand_weight;
        }
    }

    if dot_products.is_empty() {
        return Ok(vec![]);
    }

    // Step 4: Get norms for all candidates in one query
    let candidate_keys: Vec<(String, String)> = dot_products.keys().cloned().collect();

    // Build a query for all candidate norms
    let mut norm_conditions: Vec<String> = Vec::new();
    let mut norm_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;
    for (cb, cp) in &candidate_keys {
        norm_conditions.push(format!("(brain = ?{} AND path = ?{})", param_idx, param_idx + 1));
        norm_params.push(Box::new(cb.clone()));
        norm_params.push(Box::new(cp.clone()));
        param_idx += 2;
    }

    let norm_sql = format!(
        "SELECT brain, path, norm FROM doc_norms WHERE {}",
        norm_conditions.join(" OR ")
    );

    let mut norm_stmt = conn
        .prepare(&norm_sql)
        .map_err(|e| format!("prepare norms: {e}"))?;

    let norm_refs: Vec<&dyn rusqlite::types::ToSql> =
        norm_params.iter().map(|b| b.as_ref() as &dyn rusqlite::types::ToSql).collect();

    let mut candidate_norms: HashMap<(String, String), f64> = HashMap::new();
    let norm_rows = norm_stmt
        .query_map(norm_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })
        .map_err(|e| format!("query norms: {e}"))?;

    for row in norm_rows {
        let (b, p, n) = row.map_err(|e| format!("read norm row: {e}"))?;
        candidate_norms.insert((b, p), n);
    }

    // Step 5: Compute cosine similarity scores
    let mut scores: Vec<(String, String, f64)> = Vec::new();
    for ((cb, cp), dot) in &dot_products {
        let cand_norm = candidate_norms
            .get(&(cb.clone(), cp.clone()))
            .copied()
            .unwrap_or(0.0);

        // Guard against zero norms (can occur when all terms are IDF-filtered)
        if cand_norm == 0.0 {
            continue;
        }

        let cosine = dot / (query_norm * cand_norm);
        scores.push((cb.clone(), cp.clone(), cosine));
    }

    // Sort by score descending
    scores.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    scores.truncate(limit);

    // Step 6: Get category and name for the top results from brain_fts
    let mut results: Vec<SimilarDoc> = Vec::with_capacity(scores.len());

    let mut meta_stmt = conn
        .prepare("SELECT category, name FROM brain_fts WHERE brain = ?1 AND path = ?2 LIMIT 1")
        .map_err(|e| format!("prepare meta: {e}"))?;

    for (sb, sp, score) in scores {
        let (category, name) = meta_stmt
            .query_row(rusqlite::params![sb, sp], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap_or_else(|_| (String::new(), String::new()));

        results.push(SimilarDoc {
            brain: sb,
            path: sp,
            category,
            name,
            score,
        });
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi};
    use crate::tools::tfidf;

    // ---------------------------------------------------------------
    // DW-2.1: find_similar returns Vec<SimilarDoc> sorted by descending score
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_1_returns_sorted_similar_docs() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        // Create 4 docs: doc_a shares many terms with doc_b, fewer with doc_c,
        // and none with doc_d.
        let fa = create_brain_file(
            &brain_dir,
            "notes/doc-a.md",
            "---\nname: doc-a\n---\n\nRust programming systems performance concurrency safety memory",
        );
        let fb = create_brain_file(
            &brain_dir,
            "notes/doc-b.md",
            "---\nname: doc-b\n---\n\nRust programming systems performance concurrency safety memory ownership",
        );
        let fc = create_brain_file(
            &brain_dir,
            "notes/doc-c.md",
            "---\nname: doc-c\n---\n\nRust programming web framework actix tokio async runtime",
        );
        let fd = create_brain_file(
            &brain_dir,
            "ref/doc-d.md",
            "---\nname: doc-d\n---\n\nGardening botany photosynthesis chlorophyll plants soil compost",
        );

        index_file(db.conn(), "memories", "notes/doc-a.md", &fa, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/doc-b.md", &fb, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/doc-c.md", &fc, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/doc-d.md", &fd, "ref").unwrap();

        // Recompute weights now that full corpus is available
        for (b, p) in [
            ("memories", "notes/doc-a.md"),
            ("memories", "notes/doc-b.md"),
            ("memories", "notes/doc-c.md"),
            ("memories", "ref/doc-d.md"),
        ] {
            tfidf::compute_and_store_weights(db.conn(), b, p).unwrap();
        }

        let results = find_similar(db.conn(), "memories", "notes/doc-a.md", 10).unwrap();

        // Should return results (not empty)
        assert!(!results.is_empty(), "should find similar docs");

        // Results should have the expected fields
        let first = &results[0];
        assert!(!first.brain.is_empty(), "brain should be populated");
        assert!(!first.path.is_empty(), "path should be populated");
        assert!(!first.name.is_empty(), "name should be populated");
        assert!(first.score > 0.0, "score should be positive");

        // Should be sorted descending by score
        for w in results.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "results should be sorted descending: {} >= {}",
                w[0].score,
                w[1].score
            );
        }

        // doc-b should score higher than doc-c (more overlap with doc-a)
        let b_score = results.iter().find(|r| r.path == "notes/doc-b.md").map(|r| r.score);
        let c_score = results.iter().find(|r| r.path == "notes/doc-c.md").map(|r| r.score);
        assert!(
            b_score > c_score,
            "doc-b should score higher than doc-c: {:?} > {:?}",
            b_score,
            c_score
        );
    }

    // ---------------------------------------------------------------
    // DW-2.2: Candidate retrieval is sublinear
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_2_candidate_retrieval_sublinear() {
        // This test verifies the algorithm only considers docs sharing terms,
        // not all docs. We insert a doc with unique terms (no overlap with query doc)
        // and verify it does NOT appear in results.
        //
        // Need 5+ docs so shared terms have DF ratio < 50% and survive IDF filtering.
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let fa = create_brain_file(
            &brain_dir,
            "notes/query.md",
            "---\nname: query\n---\n\nRust programming systems performance concurrency",
        );
        let fb = create_brain_file(
            &brain_dir,
            "notes/overlap.md",
            "---\nname: overlap\n---\n\nRust programming language safety ownership borrowing",
        );
        let fc = create_brain_file(
            &brain_dir,
            "ref/disjoint.md",
            "---\nname: disjoint\n---\n\nGardening botany photosynthesis chlorophyll plants soil compost",
        );
        // Filler docs to dilute DF ratios so "rust" (2/5=40%) survives filtering
        let fd = create_brain_file(
            &brain_dir,
            "ref/filler1.md",
            "---\nname: filler1\n---\n\nCooking recipes fermentation sourdough baking pastry",
        );
        let fe = create_brain_file(
            &brain_dir,
            "ref/filler2.md",
            "---\nname: filler2\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations",
        );

        let files = [
            ("memories", "notes/query.md", &fa, "notes"),
            ("memories", "notes/overlap.md", &fb, "notes"),
            ("memories", "ref/disjoint.md", &fc, "ref"),
            ("memories", "ref/filler1.md", &fd, "ref"),
            ("memories", "ref/filler2.md", &fe, "ref"),
        ];
        for (b, p, f, c) in &files {
            index_file(db.conn(), b, p, f, c).unwrap();
        }
        for (b, p, _, _) in &files {
            tfidf::compute_and_store_weights(db.conn(), b, p).unwrap();
        }

        let results = find_similar(db.conn(), "memories", "notes/query.md", 10).unwrap();

        // The disjoint doc (gardening) should NOT appear (no shared terms)
        let has_disjoint = results.iter().any(|r| r.path == "ref/disjoint.md");
        assert!(
            !has_disjoint,
            "disjoint doc should not appear in results (sublinear retrieval)"
        );

        // The overlapping doc should appear
        let has_overlap = results.iter().any(|r| r.path == "notes/overlap.md");
        assert!(has_overlap, "overlapping doc should appear in results");
    }

    // ---------------------------------------------------------------
    // DW-2.3: Self-matches excluded
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_3_self_matches_excluded() {
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let fa = create_brain_file(
            &brain_dir,
            "notes/self.md",
            "---\nname: self\n---\n\nRust programming systems performance concurrency safety",
        );
        let fb = create_brain_file(
            &brain_dir,
            "notes/other.md",
            "---\nname: other\n---\n\nRust programming web framework actix tokio",
        );
        let fc = create_brain_file(
            &brain_dir,
            "ref/third.md",
            "---\nname: third\n---\n\nDifferent topic about cooking recipes baking fermentation",
        );

        index_file(db.conn(), "memories", "notes/self.md", &fa, "notes").unwrap();
        index_file(db.conn(), "memories", "notes/other.md", &fb, "notes").unwrap();
        index_file(db.conn(), "memories", "ref/third.md", &fc, "ref").unwrap();

        for (b, p) in [
            ("memories", "notes/self.md"),
            ("memories", "notes/other.md"),
            ("memories", "ref/third.md"),
        ] {
            tfidf::compute_and_store_weights(db.conn(), b, p).unwrap();
        }

        let results = find_similar(db.conn(), "memories", "notes/self.md", 10).unwrap();

        let has_self = results
            .iter()
            .any(|r| r.brain == "memories" && r.path == "notes/self.md");
        assert!(!has_self, "self-match should be excluded");
    }

    // ---------------------------------------------------------------
    // DW-2.4: L2 norms read from doc_norms, not recomputed
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_4_norms_read_from_doc_norms() {
        // We manually set a doc_norms value and verify that the cosine
        // similarity calculation uses it (not a recomputed norm).
        // Need 5+ docs so shared terms survive IDF filtering (DF ratio < 50%).
        let (db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");

        let fa = create_brain_file(
            &brain_dir,
            "notes/a.md",
            "---\nname: a\n---\n\nRust programming systems performance concurrency safety memory",
        );
        let fb = create_brain_file(
            &brain_dir,
            "notes/b.md",
            "---\nname: b\n---\n\nRust programming language safety ownership borrowing lifetimes",
        );
        let fc = create_brain_file(
            &brain_dir,
            "ref/c.md",
            "---\nname: c\n---\n\nGardening botany photosynthesis chlorophyll plants soil composting",
        );
        let fd = create_brain_file(
            &brain_dir,
            "ref/d.md",
            "---\nname: d\n---\n\nCooking recipes fermentation sourdough baking pastry desserts",
        );
        let fe = create_brain_file(
            &brain_dir,
            "ref/e.md",
            "---\nname: e\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations planets",
        );

        let files = [
            ("memories", "notes/a.md", &fa, "notes"),
            ("memories", "notes/b.md", &fb, "notes"),
            ("memories", "ref/c.md", &fc, "ref"),
            ("memories", "ref/d.md", &fd, "ref"),
            ("memories", "ref/e.md", &fe, "ref"),
        ];
        for (b, p, f, c) in &files {
            index_file(db.conn(), b, p, f, c).unwrap();
        }
        for (b, p, _, _) in &files {
            tfidf::compute_and_store_weights(db.conn(), b, p).unwrap();
        }

        // Get original score
        let results_before = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();
        let score_before = results_before
            .iter()
            .find(|r| r.path == "notes/b.md")
            .map(|r| r.score)
            .unwrap_or(0.0);

        // Double the norm in doc_norms for doc b -- this should halve the cosine score
        // if the implementation reads norms from doc_norms
        let original_norm: f64 = db
            .conn()
            .query_row(
                "SELECT norm FROM doc_norms WHERE brain = 'memories' AND path = 'notes/b.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        db.conn()
            .execute(
                "UPDATE doc_norms SET norm = ?1 WHERE brain = 'memories' AND path = 'notes/b.md'",
                rusqlite::params![original_norm * 2.0],
            )
            .unwrap();

        let results_after = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();
        let score_after = results_after
            .iter()
            .find(|r| r.path == "notes/b.md")
            .map(|r| r.score)
            .unwrap_or(0.0);

        // Score should be approximately halved (since cosine = dot / (norm_a * norm_b))
        assert!(
            score_before > 0.0,
            "should have a positive score before norm change"
        );
        let ratio = score_after / score_before;
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "doubling norm should halve score: before={score_before}, after={score_after}, ratio={ratio}"
        );
    }

    // ---------------------------------------------------------------
    // DW-2.5: Performance <100ms for 5000 docs
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_5_performance_5000_docs() {
        let (db, _tmp) = test_db();

        // Insert synthetic term_weights and doc_norms directly
        // (bypassing index_file to avoid disk I/O, per plan)
        let num_docs = 5000;
        let terms_per_doc = 20; // realistic subset of the 50 max
        let vocab_size = 500; // total unique terms in corpus

        db.conn()
            .execute_batch("BEGIN TRANSACTION")
            .unwrap();

        // Create synthetic docs with overlapping terms
        for i in 0..num_docs {
            let brain = "memories";
            let path = format!("notes/doc-{i:04}.md");

            // Insert doc_norms
            let norm = 3.5 + (i as f64 * 0.001); // realistic norm values
            db.conn()
                .execute(
                    "INSERT INTO doc_norms (brain, path, norm) VALUES (?1, ?2, ?3)",
                    rusqlite::params![brain, path, norm],
                )
                .unwrap();

            // Insert FTS row so we can get category/name
            db.conn()
                .execute(
                    "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?1, ?2, 'notes', ?3, '', '', '')",
                    rusqlite::params![path, brain, format!("doc-{i:04}")],
                )
                .unwrap();

            // Insert term_weights: each doc gets terms_per_doc terms,
            // offset so documents have overlapping but not identical term sets
            for j in 0..terms_per_doc {
                let term_idx = (i * 3 + j * 7) % vocab_size; // deterministic overlap pattern
                let term = format!("term{term_idx:03}");
                let weight = 1.0 + (j as f64 * 0.1);
                // Use INSERT OR IGNORE because two docs may map to the same
                // (brain, path, term) if the offset arithmetic repeats
                db.conn()
                    .execute(
                        "INSERT OR IGNORE INTO term_weights (brain, path, term, weight) VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![brain, path, term, weight],
                    )
                    .unwrap();
            }
        }

        db.conn()
            .execute_batch("COMMIT")
            .unwrap();

        // Benchmark
        let start = std::time::Instant::now();
        let results = find_similar(db.conn(), "memories", "notes/doc-0000.md", 10).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 100,
            "should complete in <100ms, took {}ms",
            elapsed.as_millis()
        );
        assert_eq!(
            results.len(),
            10,
            "should return exactly 10 results (limit)"
        );
    }

    // ---------------------------------------------------------------
    // DW-2.6: Cosine similarity correctness tests
    // ---------------------------------------------------------------
    #[test]
    fn test_dw_2_6_identical_docs_score_1() {
        let (db, _tmp) = test_db();

        // Insert two docs with identical term vectors
        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        for path in ["notes/a.md", "notes/b.md"] {
            db.conn()
                .execute(
                    "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?1, 'memories', 'notes', ?1, '', '', '')",
                    rusqlite::params![path],
                )
                .unwrap();

            for (term, weight) in [("rust", 2.0), ("systems", 1.5), ("performance", 1.0)] {
                db.conn()
                    .execute(
                        "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', ?1, ?2, ?3)",
                        rusqlite::params![path, term, weight],
                    )
                    .unwrap();
            }

            // L2 norm = sqrt(2^2 + 1.5^2 + 1^2) = sqrt(4 + 2.25 + 1) = sqrt(7.25)
            let norm = (4.0 + 2.25 + 1.0_f64).sqrt();
            db.conn()
                .execute(
                    "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', ?1, ?2)",
                    rusqlite::params![path, norm],
                )
                .unwrap();
        }

        db.conn().execute_batch("COMMIT").unwrap();

        let results = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();
        assert_eq!(results.len(), 1, "should find 1 similar doc (b)");
        assert!(
            (results[0].score - 1.0).abs() < 0.001,
            "identical docs should score 1.0, got {}",
            results[0].score
        );
    }

    #[test]
    fn test_dw_2_6_orthogonal_docs_score_0() {
        let (db, _tmp) = test_db();

        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        // Doc A has terms {rust, systems}
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/a.md', 'memories', 'notes', 'a', '', '', '')",
                [],
            )
            .unwrap();
        for (term, weight) in [("rust", 2.0), ("systems", 1.5)] {
            db.conn()
                .execute(
                    "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/a.md', ?1, ?2)",
                    rusqlite::params![term, weight],
                )
                .unwrap();
        }
        let norm_a = (4.0 + 2.25_f64).sqrt();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/a.md', ?1)",
                rusqlite::params![norm_a],
            )
            .unwrap();

        // Doc B has completely different terms {gardening, botany}
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/b.md', 'memories', 'notes', 'b', '', '', '')",
                [],
            )
            .unwrap();
        for (term, weight) in [("gardening", 2.0), ("botany", 1.5)] {
            db.conn()
                .execute(
                    "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/b.md', ?1, ?2)",
                    rusqlite::params![term, weight],
                )
                .unwrap();
        }
        let norm_b = (4.0 + 2.25_f64).sqrt();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/b.md', ?1)",
                rusqlite::params![norm_b],
            )
            .unwrap();

        db.conn().execute_batch("COMMIT").unwrap();

        let results = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();

        // Orthogonal docs share no terms, so the candidate set won't even include doc B
        // (term-based retrieval). The result should be empty.
        assert!(
            results.is_empty(),
            "orthogonal docs should not appear (no shared terms, score=0)"
        );
    }

    #[test]
    fn test_dw_2_6_partial_overlap_score() {
        let (db, _tmp) = test_db();

        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        // Doc A: {rust: 2.0, systems: 1.5, performance: 1.0}
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/a.md', 'memories', 'notes', 'a', '', '', '')",
                [],
            )
            .unwrap();
        for (term, weight) in [("rust", 2.0), ("systems", 1.5), ("performance", 1.0)] {
            db.conn()
                .execute(
                    "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/a.md', ?1, ?2)",
                    rusqlite::params![term, weight],
                )
                .unwrap();
        }
        let norm_a = (4.0 + 2.25 + 1.0_f64).sqrt();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/a.md', ?1)",
                rusqlite::params![norm_a],
            )
            .unwrap();

        // Doc B: {rust: 2.0, gardening: 1.5, botany: 1.0}
        // Shares only "rust" with doc A
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/b.md', 'memories', 'notes', 'b', '', '', '')",
                [],
            )
            .unwrap();
        for (term, weight) in [("rust", 2.0), ("gardening", 1.5), ("botany", 1.0)] {
            db.conn()
                .execute(
                    "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/b.md', ?1, ?2)",
                    rusqlite::params![term, weight],
                )
                .unwrap();
        }
        let norm_b = (4.0 + 2.25 + 1.0_f64).sqrt();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/b.md', ?1)",
                rusqlite::params![norm_b],
            )
            .unwrap();

        db.conn().execute_batch("COMMIT").unwrap();

        let results = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();
        assert_eq!(results.len(), 1, "should find 1 similar doc (partial overlap)");

        let score = results[0].score;
        // dot product = 2.0 * 2.0 = 4.0 (only "rust" shared)
        // cosine = 4.0 / (sqrt(7.25) * sqrt(7.25)) = 4.0 / 7.25 = 0.5517...
        let expected = 4.0 / 7.25;
        assert!(
            (score - expected).abs() < 0.01,
            "partial overlap score should be ~{expected:.4}, got {score:.4}"
        );
        assert!(score > 0.0 && score < 1.0, "partial overlap should be between 0 and 1");
    }

    #[test]
    fn test_dw_2_6_empty_term_weights_returns_empty() {
        let (db, _tmp) = test_db();

        // Insert a doc in brain_fts but with no term_weights (simulates empty/filtered doc)
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/empty.md', 'memories', 'notes', 'empty', '', '', '')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/empty.md', 0.0)",
                [],
            )
            .unwrap();

        let results = find_similar(db.conn(), "memories", "notes/empty.md", 10).unwrap();
        assert!(
            results.is_empty(),
            "doc with no term_weights should return empty results"
        );
    }

    // ---------------------------------------------------------------
    // Additional: multi-brain support
    // ---------------------------------------------------------------
    #[test]
    fn test_find_similar_cross_brain() {
        let (db, tmp) = test_db_multi();
        let mem_dir = tmp.path().join("memories");
        let docs_dir = tmp.path().join("docs");

        // Create docs in different brains with shared terms.
        // Need 5+ docs so shared terms have DF ratio < 50%.
        let fa = create_brain_file(
            &mem_dir,
            "notes/rust-mem.md",
            "---\nname: rust-mem\n---\n\nRust programming systems performance concurrency safety",
        );
        let fb = create_brain_file(
            &docs_dir,
            "rust-doc.md",
            "---\nname: rust-doc\n---\n\nRust programming language ownership borrowing lifetimes safety",
        );
        let fc = create_brain_file(
            &mem_dir,
            "ref/unrelated.md",
            "---\nname: unrelated\n---\n\nGardening botany photosynthesis chlorophyll plants composting",
        );
        let fd = create_brain_file(
            &mem_dir,
            "ref/filler1.md",
            "---\nname: filler1\n---\n\nCooking recipes fermentation sourdough baking pastry desserts",
        );
        let fe = create_brain_file(
            &docs_dir,
            "filler2.md",
            "---\nname: filler2\n---\n\nAstronomy telescopes galaxies nebula stargazing constellations planets",
        );

        let files: Vec<(&str, &str, &std::path::Path, &str)> = vec![
            ("memories", "notes/rust-mem.md", fa.as_path(), "notes"),
            ("docs", "rust-doc.md", fb.as_path(), "docs"),
            ("memories", "ref/unrelated.md", fc.as_path(), "ref"),
            ("memories", "ref/filler1.md", fd.as_path(), "ref"),
            ("docs", "filler2.md", fe.as_path(), "docs"),
        ];
        for (b, p, f, c) in &files {
            index_file(db.conn(), b, p, f, c).unwrap();
        }
        for (b, p, _, _) in &files {
            tfidf::compute_and_store_weights(db.conn(), b, p).unwrap();
        }

        let results = find_similar(db.conn(), "memories", "notes/rust-mem.md", 10).unwrap();

        // Should find the docs-brain rust doc
        let has_cross_brain = results.iter().any(|r| r.brain == "docs" && r.path == "rust-doc.md");
        assert!(has_cross_brain, "should find similar docs across brains");
    }

    // ---------------------------------------------------------------
    // Additional: zero-norm guard
    // ---------------------------------------------------------------
    #[test]
    fn test_zero_norm_candidate_excluded() {
        let (db, _tmp) = test_db();

        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        // Doc A with normal terms and norm
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/a.md', 'memories', 'notes', 'a', '', '', '')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/a.md', 'rust', 2.0)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/a.md', 2.0)",
                [],
            )
            .unwrap();

        // Doc B shares term but has zero norm (all terms were IDF-filtered)
        db.conn()
            .execute(
                "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('notes/b.md', 'memories', 'notes', 'b', '', '', '')",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', 'notes/b.md', 'rust', 1.5)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', 'notes/b.md', 0.0)",
                [],
            )
            .unwrap();

        db.conn().execute_batch("COMMIT").unwrap();

        let results = find_similar(db.conn(), "memories", "notes/a.md", 10).unwrap();

        // Doc B should be excluded due to zero norm
        let has_b = results.iter().any(|r| r.path == "notes/b.md");
        assert!(!has_b, "candidate with zero norm should be excluded");
    }

    // ---------------------------------------------------------------
    // Additional: limit parameter respected
    // ---------------------------------------------------------------
    #[test]
    fn test_limit_parameter() {
        let (db, _tmp) = test_db();

        db.conn().execute_batch("BEGIN TRANSACTION").unwrap();

        // Create 5 docs, all sharing the term "rust"
        for i in 0..5 {
            let path = format!("notes/doc-{i}.md");
            db.conn()
                .execute(
                    "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES (?1, 'memories', 'notes', ?1, '', '', '')",
                    rusqlite::params![path],
                )
                .unwrap();
            db.conn()
                .execute(
                    "INSERT INTO term_weights (brain, path, term, weight) VALUES ('memories', ?1, 'rust', ?2)",
                    rusqlite::params![path, 2.0 - (i as f64 * 0.1)],
                )
                .unwrap();
            let norm = 2.0 - (i as f64 * 0.1);
            db.conn()
                .execute(
                    "INSERT INTO doc_norms (brain, path, norm) VALUES ('memories', ?1, ?2)",
                    rusqlite::params![path, norm],
                )
                .unwrap();
        }

        db.conn().execute_batch("COMMIT").unwrap();

        // Request only 2
        let results = find_similar(db.conn(), "memories", "notes/doc-0.md", 2).unwrap();
        assert_eq!(results.len(), 2, "should respect limit parameter");
    }
}
