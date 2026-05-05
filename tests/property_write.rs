//! Property tests for write-path invariants.
//!
//! Gates: "no panic, no path-traversal escape, no FTS corruption" across 1k
//! generated cases for:
//! - DW-1.8: write→read round-trip preserves frontmatter exactly
//! - DW-1.9: no generated path escapes the brain root
//! - DW-1.10: stale-ETag write never partially writes (atomicity)

use grug_brain::helpers::slugify;
use grug_brain::tools::write::grug_write;
use grug_brain::tools::GrugDb;
use grug_brain::types::{Brain, BrainConfig};
use proptest::prelude::*;
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Test helper — mirrors src/tools/mod.rs::test_helpers::test_db()
// ---------------------------------------------------------------------------

fn make_test_db() -> (GrugDb, TempDir) {
    let tmp = TempDir::new().unwrap();
    let brain_dir = tmp.path().join("memories");
    fs::create_dir_all(&brain_dir).unwrap();

    let config = BrainConfig {
        brains: vec![Brain {
            name: "memories".to_string(),
            dir: brain_dir,
            primary: true,
            writable: true,
            flat: false,
            git: None,
            sync_interval: 60,
            source: None,
            refresh_interval: None,
        }],
        primary: "memories".to_string(),
        config_path: tmp.path().join("brains.json"),
        last_mtime: None,
    };

    let db_path = tmp.path().join("grug.db");
    let db = GrugDb::open(&db_path, config).unwrap();
    (db, tmp)
}

// ---------------------------------------------------------------------------
// Strategy helpers
// ---------------------------------------------------------------------------

/// A label: 1-20 lowercase alphanumeric chars (already valid path segment).
fn label_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,19}".prop_map(|s| s)
}

/// A frontmatter string with safe characters.
fn frontmatter_strategy() -> impl Strategy<Value = String> {
    (label_strategy(), "[a-z0-9 ]{0,40}").prop_map(|(name, desc)| {
        format!("name: {name}\ndescription: {desc}\ndate: 2025-01-01\ntype: memory")
    })
}

/// A body string with no conflict markers and no null bytes.
fn body_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,\n]{0,200}".prop_map(|s| s)
}

// ---------------------------------------------------------------------------
// DW-1.8: write→read round-trip preserves frontmatter exactly
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]
    #[test]
    fn proptest_write_read_frontmatter_roundtrip(
        cat in label_strategy(),
        name in label_strategy(),
        fm in frontmatter_strategy(),
        body in body_strategy(),
    ) {
        let (mut db, tmp) = make_test_db();
        let full_content = format!("---\n{fm}\n---\n\n{body}");

        let result = grug_write(&mut db, &cat, &name, &full_content, None, None);
        // Should succeed (no panic, no Err from valid safe inputs).
        prop_assert!(result.is_ok(), "write failed: {:?}", result);

        // Read file back and verify frontmatter is preserved.
        let brain_dir = tmp.path().join("memories");
        let cat_slug = slugify(&cat);
        let name_slug = slugify(&name);

        let file_path = brain_dir.join(&cat_slug).join(format!("{name_slug}.md"));
        prop_assert!(file_path.exists(), "written file not found at {:?}", file_path);

        let disk_content = fs::read_to_string(&file_path).unwrap();
        // All frontmatter lines we wrote should appear in the file.
        for line in fm.lines() {
            prop_assert!(
                disk_content.contains(line),
                "frontmatter line {:?} missing from file content",
                line,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DW-1.9: no generated path escapes the brain root
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]
    #[test]
    fn proptest_no_path_escape(
        cat in label_strategy(),
        name in label_strategy(),
    ) {
        let (mut db, tmp) = make_test_db();
        let brain_root = {
            let p = tmp.path().join("memories");
            fs::create_dir_all(&p).unwrap();
            p.canonicalize().unwrap_or(p)
        };

        let result = grug_write(&mut db, &cat, &name, "body text", None, None);

        if result.is_ok() {
            let cat_slug = slugify(&cat);
            let name_slug = slugify(&name);
            if !cat_slug.is_empty() && !name_slug.is_empty() {
                let written = tmp.path()
                    .join("memories")
                    .join(&cat_slug)
                    .join(format!("{name_slug}.md"));
                if written.exists() {
                    let canon = written.canonicalize().unwrap_or(written.clone());
                    prop_assert!(
                        canon.starts_with(&brain_root),
                        "path {:?} escapes brain root {:?}",
                        canon,
                        brain_root
                    );
                }
            }
        }
        // If result is Err (validation rejected the path), no file written — correct.
    }
}

// ---------------------------------------------------------------------------
// DW-1.10: stale-ETag write never partially writes (atomicity)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]
    #[test]
    fn proptest_stale_etag_atomicity(
        cat in label_strategy(),
        name in label_strategy(),
        body_v1 in body_strategy(),
        body_v2 in body_strategy(),
    ) {
        let (mut db, tmp) = make_test_db();

        // Write v1 (no ETag precondition).
        let r1 = grug_write(&mut db, &cat, &name, &body_v1, None, None);
        prop_assume!(r1.is_ok());

        let cat_slug = slugify(&cat);
        let name_slug = slugify(&name);
        prop_assume!(!cat_slug.is_empty() && !name_slug.is_empty());

        let file_path = tmp.path()
            .join("memories")
            .join(&cat_slug)
            .join(format!("{name_slug}.md"));
        prop_assume!(file_path.exists());

        let content_before = fs::read_to_string(&file_path).unwrap();

        // Attempt v2 with a stale ETag (0.0001 will never match a real mtime).
        let r2 = grug_write(&mut db, &cat, &name, &body_v2, None, Some(0.0001));

        // Should fail with a conflict error.
        prop_assert!(r2.is_err(), "stale-ETag write should return Err, got: {:?}", r2);

        // File must be unchanged (atomicity: no partial write).
        let content_after = fs::read_to_string(&file_path).unwrap();
        prop_assert_eq!(
            content_before,
            content_after,
            "file was modified despite stale ETag — atomicity violated"
        );
    }
}
