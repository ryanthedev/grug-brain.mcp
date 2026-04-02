use super::GrugDb;
use crate::tools::indexing::sync_brain;

/// Reindex a brain (or all brains) from disk.
pub fn grug_sync(db: &mut GrugDb, brain_name: Option<&str>) -> Result<String, String> {
    db.maybe_reload_config();

    let targets: Vec<_> = if let Some(name) = brain_name {
        let brains: Vec<_> = db
            .config()
            .brains
            .iter()
            .filter(|b| b.name == name)
            .cloned()
            .collect();
        if brains.is_empty() {
            return Ok(format!("unknown brain \"{name}\""));
        }
        brains
    } else {
        db.config().brains.clone()
    };

    let mut results = Vec::new();
    for brain in &targets {
        let before: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE brain = ?1",
                [&brain.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        sync_brain(db.conn(), brain)?;

        let after: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE brain = ?1",
                [&brain.name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let diff = after - before;
        let delta = if diff > 0 {
            format!(" (+{diff} new)")
        } else if diff < 0 {
            format!(" ({diff} removed)")
        } else {
            String::new()
        };

        results.push(format!("{}: {after} files{delta}", brain.name));
    }

    Ok(results.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_helpers::{create_brain_file, test_db, test_db_multi};

    #[test]
    fn test_sync_single_brain() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/a.md", "---\nname: a\n---\n\nBody");
        create_brain_file(&brain_dir, "notes/b.md", "---\nname: b\n---\n\nBody");

        let result = grug_sync(&mut db, Some("memories")).unwrap();
        assert!(result.contains("memories: 2 files"));
        assert!(result.contains("+2 new"));
    }

    #[test]
    fn test_sync_all_brains() {
        let (mut db, tmp) = test_db_multi();
        let primary_dir = tmp.path().join("memories");
        let docs_dir = tmp.path().join("docs");
        create_brain_file(&primary_dir, "notes/a.md", "---\nname: a\n---\n\nBody");
        create_brain_file(&docs_dir, "guide.md", "---\nname: guide\n---\n\nDocs");

        let result = grug_sync(&mut db, None).unwrap();
        assert!(result.contains("memories:"));
        assert!(result.contains("docs:"));
    }

    #[test]
    fn test_sync_unknown_brain() {
        let (mut db, _tmp) = test_db();
        let result = grug_sync(&mut db, Some("ghost")).unwrap();
        assert!(result.contains("unknown brain"));
    }

    #[test]
    fn test_sync_idempotent() {
        let (mut db, tmp) = test_db();
        let brain_dir = tmp.path().join("memories");
        create_brain_file(&brain_dir, "notes/a.md", "---\nname: a\n---\n\nBody");

        let r1 = grug_sync(&mut db, None).unwrap();
        assert!(r1.contains("+1 new"));

        let r2 = grug_sync(&mut db, None).unwrap();
        // Second sync should show no changes
        assert!(r2.contains("1 files"));
        assert!(!r2.contains("new"));
    }
}
