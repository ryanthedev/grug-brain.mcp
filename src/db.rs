use rusqlite::{Connection, Result as SqlResult};
use std::path::Path;

pub const SCHEMA_VERSION: i32 = 6;

/// Initialize the grug database at the given path.
/// Creates all tables if they don't exist.
/// If schema version < current, drops and recreates all tables.
pub fn init_db(db_path: &Path) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;

    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)",
        [],
    )?;

    // Check schema version
    let cur_version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok();

    let needs_recreate = match &cur_version {
        None => true,
        Some(v) => v.parse::<i32>().unwrap_or(0) < SCHEMA_VERSION,
    };

    if needs_recreate {
        conn.execute_batch(
            "DROP TABLE IF EXISTS files;
             DROP TABLE IF EXISTS brain_fts;
             DROP TABLE IF EXISTS memories_fts;
             DROP TABLE IF EXISTS docs_fts;
             DROP TABLE IF EXISTS term_weights;
             DROP TABLE IF EXISTS doc_norms;
             DROP TABLE IF EXISTS dream_log;
             DROP TABLE IF EXISTS cross_links;",
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('schema_version', ?1)",
            [&SCHEMA_VERSION.to_string()],
        )?;
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS files (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            mtime REAL NOT NULL,
            PRIMARY KEY (brain, path)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS brain_fts USING fts5(
            path UNINDEXED, brain UNINDEXED, category, name, date UNINDEXED, description, body,
            tokenize = 'porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS dream_log (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            reviewed_at TEXT NOT NULL,
            mtime_at_review REAL NOT NULL,
            PRIMARY KEY (brain, path)
        );

        CREATE TABLE IF NOT EXISTS cross_links (
            brain_a TEXT NOT NULL,
            path_a TEXT NOT NULL,
            brain_b TEXT NOT NULL,
            path_b TEXT NOT NULL,
            score REAL NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (brain_a, path_a, brain_b, path_b)
        );

        CREATE TABLE IF NOT EXISTS term_weights (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            term TEXT NOT NULL,
            weight REAL NOT NULL,
            PRIMARY KEY (brain, path, term)
        );
        CREATE INDEX IF NOT EXISTS idx_term_weights_term ON term_weights (term);

        CREATE TABLE IF NOT EXISTS doc_norms (
            brain TEXT NOT NULL,
            path TEXT NOT NULL,
            norm REAL NOT NULL,
            PRIMARY KEY (brain, path)
        );",
    )?;

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_schema_creation() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = init_db(tmp.path()).unwrap();

        // Verify meta table has schema_version = 6
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "6");

        // Verify files table works
        conn.execute(
            "INSERT INTO files (brain, path, mtime) VALUES ('test', '/a.md', 1.0)",
            [],
        )
        .unwrap();
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify brain_fts works
        conn.execute(
            "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('/a.md', 'test', 'cat', 'name', '2025-01-01', 'desc', 'body text')",
            [],
        )
        .unwrap();
        let fts_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE brain_fts MATCH 'body'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fts_count, 1);

        // Verify dream_log works
        conn.execute(
            "INSERT INTO dream_log (brain, path, reviewed_at, mtime_at_review) VALUES ('test', '/a.md', '2025-01-01', 1.0)",
            [],
        )
        .unwrap();

        // Verify cross_links works
        conn.execute(
            "INSERT INTO cross_links (brain_a, path_a, brain_b, path_b, score, created_at) VALUES ('a', '/a.md', 'b', '/b.md', 0.5, '2025-01-01')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn test_schema_migration() {
        let tmp = NamedTempFile::new().unwrap();

        // Create a database with old schema version
        {
            let conn = Connection::open(tmp.path()).unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '3')",
                [],
            )
            .unwrap();
            // Create an old-style files table
            conn.execute(
                "CREATE TABLE files (brain TEXT, path TEXT, old_col TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files (brain, path, old_col) VALUES ('test', '/a.md', 'old')",
                [],
            )
            .unwrap();
        }

        // Re-init should drop and recreate
        let conn = init_db(tmp.path()).unwrap();

        // Schema version should be 6 now
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "6");

        // Old data should be gone (table was dropped and recreated)
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_fts5_porter_stemming() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = init_db(tmp.path()).unwrap();

        conn.execute(
            "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('/a.md', 'test', 'cat', 'testing', '2025-01-01', 'desc', 'running quickly')",
            [],
        )
        .unwrap();

        // Porter stemming: "run" should match "running"
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE brain_fts MATCH 'run'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "porter stemming should match 'run' to 'running'");

        // "running" should also match "runs" (both stem to "run")
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM brain_fts WHERE brain_fts MATCH 'runs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "porter stemming: 'runs' and 'running' both stem to 'run'");
    }

    #[test]
    fn test_fts5_highlight() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = init_db(tmp.path()).unwrap();

        conn.execute(
            "INSERT INTO brain_fts (path, brain, category, name, date, description, body) VALUES ('/a.md', 'test', 'cat', 'name', '2025-01-01', 'a test description', 'body text')",
            [],
        )
        .unwrap();

        // Verify highlight function works with same markers as JS
        let snippet: String = conn
            .query_row(
                "SELECT highlight(brain_fts, 5, '>>>', '<<<') FROM brain_fts WHERE brain_fts MATCH 'test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(snippet.contains(">>>"), "highlight markers should be present: {snippet}");
    }

    #[test]
    fn test_dw_1_1_term_weights_table_exists() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = init_db(tmp.path()).unwrap();

        // Verify term_weights table works with correct schema
        conn.execute(
            "INSERT INTO term_weights (brain, path, term, weight) VALUES ('test', '/a.md', 'rust', 0.95)",
            [],
        )
        .unwrap();

        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM term_weights", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);

        // Verify primary key constraint (brain, path, term)
        let dup = conn.execute(
            "INSERT INTO term_weights (brain, path, term, weight) VALUES ('test', '/a.md', 'rust', 0.5)",
            [],
        );
        assert!(dup.is_err(), "duplicate PK should fail");

        // Verify term index exists by querying on term
        let weight: f64 = conn
            .query_row(
                "SELECT weight FROM term_weights WHERE term = 'rust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!((weight - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_dw_1_1_doc_norms_table_exists() {
        let tmp = NamedTempFile::new().unwrap();
        let conn = init_db(tmp.path()).unwrap();

        // Verify doc_norms table works with correct schema
        conn.execute(
            "INSERT INTO doc_norms (brain, path, norm) VALUES ('test', '/a.md', 1.5)",
            [],
        )
        .unwrap();

        let norm: f64 = conn
            .query_row(
                "SELECT norm FROM doc_norms WHERE brain = 'test' AND path = '/a.md'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!((norm - 1.5).abs() < 0.001);

        // Verify primary key constraint (brain, path)
        let dup = conn.execute(
            "INSERT INTO doc_norms (brain, path, norm) VALUES ('test', '/a.md', 2.0)",
            [],
        );
        assert!(dup.is_err(), "duplicate PK should fail");
    }

    #[test]
    fn test_dw_1_2_migration_drops_all_tables() {
        let tmp = NamedTempFile::new().unwrap();

        // Create a database with old schema version and populate tables
        {
            let conn = Connection::open(tmp.path()).unwrap();
            conn.execute(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', '5')",
                [],
            )
            .unwrap();
            // Create tables that should be dropped on migration
            conn.execute_batch(
                "CREATE TABLE files (brain TEXT, path TEXT);
                 CREATE TABLE dream_log (brain TEXT, path TEXT);
                 CREATE TABLE cross_links (brain_a TEXT, path_a TEXT, brain_b TEXT, path_b TEXT, score REAL, created_at TEXT);
                 CREATE TABLE term_weights (brain TEXT, path TEXT, term TEXT, weight REAL);
                 CREATE TABLE doc_norms (brain TEXT, path TEXT, norm REAL);",
            )
            .unwrap();
            conn.execute("INSERT INTO dream_log VALUES ('test', '/a.md')", [])
                .unwrap();
            conn.execute(
                "INSERT INTO cross_links VALUES ('a', '/a.md', 'b', '/b.md', 0.5, '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO term_weights VALUES ('test', '/a.md', 'rust', 0.9)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO doc_norms VALUES ('test', '/a.md', 1.5)",
                [],
            )
            .unwrap();
        }

        // Re-init should drop ALL tables including dream_log, cross_links, term_weights, doc_norms
        let conn = init_db(tmp.path()).unwrap();

        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "6");

        // All old data should be gone
        let dream_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM dream_log", [], |row| row.get(0))
            .unwrap();
        assert_eq!(dream_count, 0, "dream_log should be empty after migration");

        let cross_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM cross_links", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cross_count, 0, "cross_links should be empty after migration");

        let tw_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM term_weights", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tw_count, 0, "term_weights should be empty after migration");

        let dn_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM doc_norms", [], |row| row.get(0))
            .unwrap();
        assert_eq!(dn_count, 0, "doc_norms should be empty after migration");
    }

    #[test]
    fn test_idempotent_init() {
        let tmp = NamedTempFile::new().unwrap();

        // Init twice -- should not error
        let conn1 = init_db(tmp.path()).unwrap();
        conn1.execute(
            "INSERT INTO files (brain, path, mtime) VALUES ('test', '/a.md', 1.0)",
            [],
        )
        .unwrap();
        drop(conn1);

        let conn2 = init_db(tmp.path()).unwrap();
        // Data should still be there (schema version matches, no drop)
        let count: i32 = conn2
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
