use crate::types::{Brain, BrainConfig};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Expand `~` to the user's home directory.
pub fn expand_home(path: &str) -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());

    if path == "~" {
        PathBuf::from(&home)
    } else if let Some(rest) = path.strip_prefix("~/") {
        PathBuf::from(&home).join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn ensure_dir(path: &Path) {
    if !path.exists() {
        fs::create_dir_all(path).ok();
    }
}

/// Load brain configuration from `~/.grug-brain/brains.json` (or GRUG_CONFIG env).
///
/// Validation rules (matching JS server.js):
/// - Config must be a JSON array
/// - Each brain must have name (string) and dir (string)
/// - Names must be unique
/// - Exactly one brain must be marked primary
/// - flat defaults to false; writable defaults to true (false for flat brains)
/// - Brains whose directories don't exist are filtered out
pub fn load_brains() -> Result<BrainConfig, String> {
    load_brains_from(None)
}

/// Load brains from a specific config path (for testing).
pub fn load_brains_from(config_override: Option<&Path>) -> Result<BrainConfig, String> {
    let config_path = match config_override {
        Some(p) => p.to_path_buf(),
        None => {
            if let Ok(env_path) = std::env::var("GRUG_CONFIG") {
                PathBuf::from(env_path)
            } else {
                expand_home("~/.grug-brain/brains.json")
            }
        }
    };

    if !config_path.exists() {
        return create_default_config(&config_path);
    }

    let raw_text = fs::read_to_string(&config_path)
        .map_err(|e| format!("grug: failed to read {}: {e}", config_path.display()))?;

    let raw: Value = serde_json::from_str(&raw_text)
        .map_err(|e| format!("grug: failed to parse {}: {e}", config_path.display()))?;

    let arr = raw
        .as_array()
        .ok_or_else(|| format!("grug: {} must be a JSON array", config_path.display()))?;

    let mut brains = Vec::new();

    for (i, entry) in arr.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("grug: brain[{i}] missing required \"name\" field"))?
            .to_string();

        let dir_raw = entry
            .get("dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("grug: brain \"{name}\" missing required \"dir\" field"))?;

        let flat = entry
            .get("flat")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let writable = match entry.get("writable").and_then(|v| v.as_bool()) {
            Some(w) => w,
            None => !flat, // default: true for normal, false for flat
        };

        let primary = entry
            .get("primary")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let git = entry
            .get("git")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let sync_interval = entry
            .get("syncInterval")
            .and_then(|v| v.as_u64())
            .unwrap_or(60);

        let source = entry
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let dir = fs::canonicalize(expand_home(dir_raw))
            .unwrap_or_else(|_| expand_home(dir_raw));

        brains.push(Brain {
            name,
            dir,
            primary,
            writable,
            flat,
            git,
            sync_interval,
            source,
        });
    }

    // Validate: unique names
    let mut names = HashSet::new();
    for brain in &brains {
        if !names.insert(&brain.name) {
            return Err(format!(
                "grug: duplicate brain name \"{}\" in {}",
                brain.name,
                config_path.display()
            ));
        }
    }

    // Validate: exactly one primary
    let primaries: Vec<&Brain> = brains.iter().filter(|b| b.primary).collect();
    if primaries.is_empty() {
        return Err(format!(
            "grug: no brain marked \"primary: true\" in {}",
            config_path.display()
        ));
    }
    if primaries.len() > 1 {
        let names: Vec<&str> = primaries.iter().map(|b| b.name.as_str()).collect();
        return Err(format!(
            "grug: multiple brains marked \"primary: true\" in {}: {}",
            config_path.display(),
            names.join(", ")
        ));
    }

    let primary_name = primaries[0].name.clone();

    // Filter out brains whose directories don't exist
    brains.retain(|b| b.dir.exists());

    // Verify the primary brain survived filtering
    if !brains.iter().any(|b| b.name == primary_name) {
        return Err(format!(
            "grug: primary brain \"{primary_name}\" directory does not exist"
        ));
    }

    Ok(BrainConfig {
        brains,
        primary: primary_name,
        config_path,
        last_mtime: None,
    })
}

fn create_default_config(config_path: &Path) -> Result<BrainConfig, String> {
    let default_dir = expand_home("~/.grug-brain/memories");
    ensure_dir(&default_dir);

    let default_config = serde_json::json!([
        {
            "name": "memories",
            "dir": default_dir.to_str().unwrap_or("~/.grug-brain/memories"),
            "primary": true,
            "writable": true
        }
    ]);

    if let Some(parent) = config_path.parent() {
        ensure_dir(parent);
    }

    fs::write(
        config_path,
        serde_json::to_string_pretty(&default_config).unwrap() + "\n",
    )
    .map_err(|e| format!("grug: failed to write default config: {e}"))?;

    let brain = Brain {
        name: "memories".to_string(),
        dir: default_dir,
        primary: true,
        writable: true,
        flat: false,
        git: None,
        sync_interval: 60,
        source: None,
    };

    Ok(BrainConfig {
        brains: vec![brain],
        primary: "memories".to_string(),
        config_path: config_path.to_path_buf(),
        last_mtime: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &Path, json: &str) -> PathBuf {
        let config_path = dir.join("brains.json");
        fs::write(&config_path, json).unwrap();
        config_path
    }

    fn make_brain_dir(dir: &Path, name: &str) -> PathBuf {
        let brain_dir = dir.join(name);
        fs::create_dir_all(&brain_dir).unwrap();
        brain_dir
    }

    #[test]
    fn test_load_valid_config() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_a = make_brain_dir(dir, "alpha");
        let brain_b = make_brain_dir(dir, "beta");

        let json = format!(
            r#"[
                {{"name": "alpha", "dir": "{}", "primary": true, "writable": true}},
                {{"name": "beta", "dir": "{}", "writable": true}}
            ]"#,
            brain_a.display(),
            brain_b.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert_eq!(cfg.brains.len(), 2);
        assert_eq!(cfg.primary, "alpha");
    }

    #[test]
    fn test_missing_name() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let json = r#"[{"dir": "/tmp/foo", "primary": true}]"#;
        let config_path = write_config(dir, json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("missing required \"name\""), "got: {err}");
    }

    #[test]
    fn test_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let json = r#"[{"name": "foo", "primary": true}]"#;
        let config_path = write_config(dir, json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("missing required \"dir\""), "got: {err}");
    }

    #[test]
    fn test_duplicate_names() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "memories");
        let json = format!(
            r#"[
                {{"name": "foo", "dir": "{}", "primary": true}},
                {{"name": "foo", "dir": "{}"}}
            ]"#,
            brain_dir.display(),
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("duplicate brain name"), "got: {err}");
    }

    #[test]
    fn test_no_primary() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "memories");
        let json = format!(
            r#"[{{"name": "foo", "dir": "{}"}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("no brain marked \"primary: true\""), "got: {err}");
    }

    #[test]
    fn test_multiple_primaries() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let a = make_brain_dir(dir, "a");
        let b = make_brain_dir(dir, "b");
        let json = format!(
            r#"[
                {{"name": "a", "dir": "{}", "primary": true}},
                {{"name": "b", "dir": "{}", "primary": true}}
            ]"#,
            a.display(),
            b.display()
        );
        let config_path = write_config(dir, &json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("multiple brains marked \"primary: true\""), "got: {err}");
    }

    #[test]
    fn test_flat_writable_default() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "docs");
        let json = format!(
            r#"[{{"name": "docs", "dir": "{}", "primary": true, "flat": true}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert!(!cfg.brains[0].writable); // flat defaults to not writable
        assert!(cfg.brains[0].flat);
    }

    #[test]
    fn test_flat_writable_explicit() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "docs");
        let json = format!(
            r#"[{{"name": "docs", "dir": "{}", "primary": true, "flat": true, "writable": true}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert!(cfg.brains[0].writable); // explicit overrides flat default
    }

    #[test]
    fn test_home_expansion() {
        // We can't easily test ~ expansion in isolation, but we can test expand_home
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_home("~"), PathBuf::from(&home));
        assert_eq!(
            expand_home("~/test-brain"),
            PathBuf::from(&home).join("test-brain")
        );
        assert_eq!(expand_home("/absolute/path"), PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_missing_dir_filtered() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let existing = make_brain_dir(dir, "exists");
        let json = format!(
            r#"[
                {{"name": "exists", "dir": "{}", "primary": true}},
                {{"name": "gone", "dir": "/nonexistent/path/xyzzy"}}
            ]"#,
            existing.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert_eq!(cfg.brains.len(), 1);
        assert_eq!(cfg.brains[0].name, "exists");
    }

    #[test]
    fn test_default_config_creation() {
        let tmp = TempDir::new().unwrap();
        let config_path = tmp.path().join("subdir").join("brains.json");
        // Config file doesn't exist yet
        assert!(!config_path.exists());
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert!(config_path.exists()); // created
        assert_eq!(cfg.brains.len(), 1);
        assert_eq!(cfg.brains[0].name, "memories");
        assert!(cfg.brains[0].primary);
    }

    #[test]
    fn test_sync_interval_default() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "mem");
        let json = format!(
            r#"[{{"name": "mem", "dir": "{}", "primary": true}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert_eq!(cfg.brains[0].sync_interval, 60);
    }

    #[test]
    fn test_sync_interval_custom() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "mem");
        let json = format!(
            r#"[{{"name": "mem", "dir": "{}", "primary": true, "syncInterval": 300}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert_eq!(cfg.brains[0].sync_interval, 300);
    }

    #[test]
    fn test_source_field_preserved() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let brain_dir = make_brain_dir(dir, "docs");
        let json = format!(
            r#"[{{"name": "docs", "dir": "{}", "primary": true, "flat": true, "source": "github:org/repo/path"}}]"#,
            brain_dir.display()
        );
        let config_path = write_config(dir, &json);
        let cfg = load_brains_from(Some(&config_path)).unwrap();
        assert_eq!(
            cfg.brains[0].source.as_deref(),
            Some("github:org/repo/path")
        );
    }

    #[test]
    fn test_not_array() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let json = r#"{"name": "foo"}"#;
        let config_path = write_config(dir, json);
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("must be a JSON array"), "got: {err}");
    }

    #[test]
    fn test_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let config_path = write_config(dir, "not json at all");
        let err = load_brains_from(Some(&config_path)).unwrap_err();
        assert!(err.contains("failed to parse"), "got: {err}");
    }
}
