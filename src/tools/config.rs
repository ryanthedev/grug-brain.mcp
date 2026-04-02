use super::GrugDb;
use crate::config::expand_home;
use crate::tools::indexing::{remove_file, sync_brain};
use std::fs;

/// Manage brain configuration. list/add/remove actions.
pub fn grug_config(
    db: &mut GrugDb,
    action: &str,
    name: Option<&str>,
    dir: Option<&str>,
    primary: Option<bool>,
    writable: Option<bool>,
    flat: Option<bool>,
    git: Option<&str>,
    sync_interval: Option<u64>,
    source: Option<&str>,
    refresh_interval: Option<u64>,
) -> Result<String, String> {
    db.maybe_reload_config();

    match action {
        "list" => config_list(db),
        "add" => config_add(
            db,
            name,
            dir,
            primary,
            writable,
            flat,
            git,
            sync_interval,
            source,
            refresh_interval,
        ),
        "remove" => config_remove(db, name),
        _ => Ok(format!("unknown action \"{action}\"")),
    }
}

fn config_list(db: &GrugDb) -> Result<String, String> {
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
        if let Some(ref g) = b.git {
            flags.push(format!("git:{g}"));
        }
        if !b.writable {
            if let (Some(_source), Some(ri)) = (&b.source, b.refresh_interval) {
                flags.push(format!("refresh:{ri}s"));
            }
        }
        // sync-active/refresh-active flags skipped (Phase 4 concept)

        lines.push(format!("  {}  ({} files, {})", b.name, count, flags.join(", ")));
    }

    Ok(format!("{} brains\n\n{}", brains.len(), lines.join("\n")))
}

fn config_add(
    db: &mut GrugDb,
    name: Option<&str>,
    dir: Option<&str>,
    primary: Option<bool>,
    writable: Option<bool>,
    flat: Option<bool>,
    git: Option<&str>,
    sync_interval: Option<u64>,
    source: Option<&str>,
    refresh_interval: Option<u64>,
) -> Result<String, String> {
    let name = name.ok_or("add requires: name")?;
    let dir_raw = dir.ok_or("add requires: dir")?;

    // Validate name format
    let name_re = regex::Regex::new(r"^[a-z0-9][a-z0-9-]*$").unwrap();
    if !name_re.is_match(name) {
        return Ok(format!(
            "invalid brain name \"{name}\": use lowercase letters, digits, and hyphens only"
        ));
    }

    // Read current brains.json from disk
    let existing = read_brains_json(&db.config().config_path)?;

    // Reject duplicate names
    if existing
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .any(|b| b.get("name").and_then(|v| v.as_str()) == Some(name))
    {
        return Ok(format!("brain \"{name}\" already exists"));
    }

    // Reject multiple primaries
    let is_primary = primary.unwrap_or(false);
    if is_primary
        && existing
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|b| b.get("primary").and_then(|v| v.as_bool()).unwrap_or(false))
    {
        return Ok(
            "a primary brain already exists -- set primary: false or remove the existing primary first"
                .to_string(),
        );
    }

    let resolved_dir = fs::canonicalize(expand_home(dir_raw))
        .unwrap_or_else(|_| expand_home(dir_raw));

    // Ensure directory exists
    if !resolved_dir.exists() {
        fs::create_dir_all(&resolved_dir).ok();
    }

    let is_flat = flat.unwrap_or(false);
    let is_writable = writable.unwrap_or(!is_flat);

    let mut entry = serde_json::json!({
        "name": name,
        "dir": resolved_dir.to_string_lossy(),
        "primary": is_primary,
        "writable": is_writable,
        "flat": is_flat,
        "git": git,
        "syncInterval": sync_interval.unwrap_or(60),
    });

    if let Some(s) = source {
        entry["source"] = serde_json::Value::String(s.to_string());
    }
    if let Some(ri) = refresh_interval {
        entry["refreshInterval"] = serde_json::Value::Number(serde_json::Number::from(ri));
    }

    // Append and write
    let mut arr = match existing {
        serde_json::Value::Array(a) => a,
        _ => vec![],
    };
    arr.push(entry);

    write_brains_json(&db.config().config_path, &serde_json::Value::Array(arr))?;

    // Force config reload
    db.config_mut().last_mtime = None;
    db.maybe_reload_config();

    // Sync the new brain if it loaded
    if let Some(brain) = db.config().get(name).cloned() {
        let _ = sync_brain(db.conn(), &brain);
    }

    Ok(format!(
        "added brain \"{name}\" -- dir: {}",
        resolved_dir.display()
    ))
}

fn config_remove(db: &mut GrugDb, name: Option<&str>) -> Result<String, String> {
    let name = name.ok_or("remove requires: name")?;

    let existing = read_brains_json(&db.config().config_path)?;

    let arr = existing.as_array().ok_or("config is not an array")?;

    let entry = arr
        .iter()
        .find(|b| b.get("name").and_then(|v| v.as_str()) == Some(name));

    let entry = match entry {
        Some(e) => e.clone(),
        None => return Ok(format!("no brain named \"{name}\"")),
    };

    if entry
        .get("primary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(format!("cannot remove the primary brain \"{name}\""));
    }

    let entry_dir = entry
        .get("dir")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();

    // Remove from FTS index
    let indexed_paths: Vec<String> = {
        let mut stmt = db
            .conn()
            .prepare("SELECT path FROM files WHERE brain = ?1")
            .map_err(|e| format!("prepare: {e}"))?;
        stmt.query_map([name], |row| row.get(0))
            .map_err(|e| format!("query: {e}"))?
            .filter_map(|r| r.ok())
            .collect()
    };
    for rel_path in &indexed_paths {
        remove_file(db.conn(), name, rel_path)?;
    }

    // Write updated config
    let updated: Vec<&serde_json::Value> = arr
        .iter()
        .filter(|b| b.get("name").and_then(|v| v.as_str()) != Some(name))
        .collect();

    write_brains_json(
        &db.config().config_path,
        &serde_json::Value::Array(updated.into_iter().cloned().collect()),
    )?;

    // Force config reload
    db.config_mut().last_mtime = None;
    db.maybe_reload_config();

    Ok(format!(
        "removed brain \"{name}\" from config (files preserved at {entry_dir})"
    ))
}

fn read_brains_json(config_path: &std::path::Path) -> Result<serde_json::Value, String> {
    if !config_path.exists() {
        return Ok(serde_json::Value::Array(vec![]));
    }
    let raw = fs::read_to_string(config_path)
        .map_err(|e| format!("cannot read config: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("cannot read config: failed to parse: {e}"))
}

fn write_brains_json(
    config_path: &std::path::Path,
    value: &serde_json::Value,
) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).ok();
        }
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| format!("serialize: {e}"))?;
    fs::write(config_path, format!("{json}\n"))
        .map_err(|e| format!("failed to write config: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::indexing::index_file;
    use crate::tools::test_helpers::{create_brain_file, test_db};

    fn setup_config_file(db: &GrugDb) {
        // Write a valid brains.json so config operations can read it
        let primary = db.config().primary_brain();
        let config = serde_json::json!([{
            "name": primary.name,
            "dir": primary.dir.to_string_lossy(),
            "primary": true,
            "writable": true,
        }]);
        fs::write(
            &db.config().config_path,
            serde_json::to_string_pretty(&config).unwrap() + "\n",
        )
        .unwrap();
    }

    #[test]
    fn test_config_list() {
        let (mut db, _tmp) = test_db();
        let result = grug_config(&mut db, "list", None, None, None, None, None, None, None, None, None).unwrap();
        assert!(result.contains("1 brains"));
        assert!(result.contains("memories"));
        assert!(result.contains("primary"));
    }

    #[test]
    fn test_config_add() {
        let (mut db, tmp) = test_db();
        setup_config_file(&db);

        let new_dir = tmp.path().join("new-brain");
        fs::create_dir_all(&new_dir).unwrap();

        let result = grug_config(
            &mut db,
            "add",
            Some("new-brain"),
            Some(new_dir.to_str().unwrap()),
            None,
            Some(true),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("added brain \"new-brain\""));

        // Verify config was updated
        let config_content = fs::read_to_string(&db.config().config_path).unwrap();
        assert!(config_content.contains("new-brain"));
    }

    #[test]
    fn test_config_add_duplicate() {
        let (mut db, _tmp) = test_db();
        setup_config_file(&db);

        let result = grug_config(
            &mut db,
            "add",
            Some("memories"),
            Some("/tmp/whatever"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("already exists"));
    }

    #[test]
    fn test_config_add_invalid_name() {
        let (mut db, _tmp) = test_db();
        let result = grug_config(
            &mut db,
            "add",
            Some("Bad Name!"),
            Some("/tmp"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("invalid brain name"));
    }

    #[test]
    fn test_config_add_multiple_primaries() {
        let (mut db, _tmp) = test_db();
        setup_config_file(&db);

        let result = grug_config(
            &mut db,
            "add",
            Some("second"),
            Some("/tmp"),
            Some(true),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("primary brain already exists"));
    }

    #[test]
    fn test_config_remove() {
        let (mut db, tmp) = test_db();

        // Setup config with two brains
        let second_dir = tmp.path().join("second");
        fs::create_dir_all(&second_dir).unwrap();
        let config = serde_json::json!([
            {
                "name": "memories",
                "dir": tmp.path().join("memories").to_string_lossy(),
                "primary": true,
                "writable": true,
            },
            {
                "name": "second",
                "dir": second_dir.to_string_lossy(),
                "writable": true,
            }
        ]);
        fs::write(
            &db.config().config_path,
            serde_json::to_string_pretty(&config).unwrap() + "\n",
        )
        .unwrap();

        // Index a file in the second brain
        let f = create_brain_file(&second_dir, "cat/test.md", "---\nname: test\n---\n\nBody");
        index_file(db.conn(), "second", "cat/test.md", &f, "cat").unwrap();

        let result = grug_config(
            &mut db,
            "remove",
            Some("second"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("removed brain \"second\""));
        assert!(result.contains("files preserved"));

        // Verify DB cleanup
        let count: i32 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM files WHERE brain = 'second'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_config_remove_primary() {
        let (mut db, _tmp) = test_db();
        setup_config_file(&db);

        let result = grug_config(
            &mut db,
            "remove",
            Some("memories"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("cannot remove the primary brain"));
    }

    #[test]
    fn test_config_remove_nonexistent() {
        let (mut db, _tmp) = test_db();
        setup_config_file(&db);

        let result = grug_config(
            &mut db,
            "remove",
            Some("ghost"),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("no brain named"));
    }

    #[test]
    fn test_config_unknown_action() {
        let (mut db, _tmp) = test_db();
        let result = grug_config(
            &mut db,
            "wipe",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.contains("unknown action"));
    }
}
