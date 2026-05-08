//! `impl WritePort for GrugDb` — every persisted-state mutation.
//!
//! MCP-shaped methods (`grug_write`/`grug_delete`/`grug_update`) delegate to
//! `crate::tools::*`. HTTP-shaped methods (`memory_*_json`) contain the
//! data-access logic directly (moved here from `handlers.rs` in Phase 3).

use crate::domain::ports::WritePort;
use crate::helpers::validate_memory_path;
use crate::tools::update::EditEntry;
use crate::tools::GrugDb;
use serde_json::json;

// ---------------------------------------------------------------------------
// Private helpers (moved from handlers.rs in Phase 3)
// ---------------------------------------------------------------------------

/// Split a relative path `"category/name"` or `"category/name.md"` into
/// `(category, stem)` where stem has no `.md` extension. Returns Err if the
/// format is invalid.
fn split_rel_path(rel_path: &str) -> Result<(String, String), String> {
    let stripped = rel_path.strip_suffix(".md").unwrap_or(rel_path);
    if let Some(pos) = stripped.rfind('/') {
        let cat = stripped[..pos].to_string();
        let name = stripped[pos + 1..].to_string();
        if cat.is_empty() || name.is_empty() {
            return Err(format!("invalid path: {rel_path:?}"));
        }
        Ok((cat, name))
    } else {
        Err(format!("path must be 'category/name', got: {rel_path:?}"))
    }
}

/// Read the current mtime for a path from the `files` table.
fn read_mtime(db: &mut GrugDb, brain_name: &str, rel_path: &str) -> f64 {
    db.conn()
        .query_row(
            "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
            rusqlite::params![brain_name, rel_path],
            |row| row.get(0),
        )
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// WritePort impl
// ---------------------------------------------------------------------------

impl WritePort for GrugDb {
    fn grug_write(
        &mut self,
        category: &str,
        path: &str,
        content: &str,
        brain: Option<&str>,
        if_match_mtime: Option<f64>,
    ) -> Result<String, String> {
        crate::tools::write::grug_write(self, category, path, content, brain, if_match_mtime)
    }

    fn grug_delete(
        &mut self,
        category: &str,
        path: &str,
        brain: Option<&str>,
        hard: bool,
    ) -> Result<String, String> {
        crate::tools::delete::grug_delete(self, category, path, brain, hard)
    }

    fn grug_update(
        &mut self,
        category: &str,
        path: &str,
        edits: &[EditEntry],
        brain: Option<&str>,
    ) -> Result<String, String> {
        crate::tools::update::grug_update(self, category, path, edits, brain)
    }

    fn memory_write_json(
        &mut self,
        brain_name: &str,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
        if_match_etag: f64,
        attempted_body: &str,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let brain = self.resolve_brain(Some(brain_name))?.clone();

        if !brain.writable {
            let v = json!({"error": "read-only brain", "brain": brain.name});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let (category, stem) = split_rel_path(rel_path)?;
        validate_memory_path(&category)?;
        validate_memory_path(&stem)?;

        let file_content = if let Some(fm) = frontmatter {
            if fm.trim().is_empty() {
                body.to_string()
            } else {
                format!("---\n{}\n---\n\n{}", fm.trim_end(), body)
            }
        } else {
            body.to_string()
        };

        let canonical_rel = format!("{category}/{stem}.md");
        let file_path = brain.dir.join(&category).join(format!("{stem}.md"));

        if !file_path.exists() {
            let v = json!({"error": "not found", "path": canonical_rel});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let result = crate::tools::write::grug_write(
            self,
            &category,
            &stem,
            &file_content,
            Some(brain_name),
            Some(if_match_etag),
        );

        match result {
            Err(conflict_json) => {
                let inner: serde_json::Value = serde_json::from_str(&conflict_json)
                    .unwrap_or(json!({"error": "conflict"}));
                let current_etag = inner
                    .get("current_mtime")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let current_body = inner
                    .get("current_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let v = json!({
                    "error": "conflict",
                    "current_etag": current_etag,
                    "current_body": current_body,
                    "attempted_body": attempted_body,
                });
                Ok(serde_json::to_string(&v).map_err(|e| e.to_string())?)
            }
            Ok(_) => {
                let new_mtime = read_mtime(self, &brain.name, &canonical_rel);
                let v = json!({"ok": true, "etag": new_mtime});
                serde_json::to_string(&v).map_err(|e| e.to_string())
            }
        }
    }

    fn memory_create_json(
        &mut self,
        brain_name: Option<&str>,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let brain = self.resolve_brain(brain_name)?.clone();

        if !brain.writable {
            let v = json!({"error": "read-only brain", "brain": brain.name});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let (category, stem) = split_rel_path(rel_path)?;
        validate_memory_path(&category)?;
        validate_memory_path(&stem)?;

        let canonical_rel = format!("{category}/{stem}.md");
        let file_path = brain.dir.join(&category).join(format!("{stem}.md"));
        if file_path.exists() {
            let v = json!({"error": "duplicate path", "path": canonical_rel});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let file_content = if let Some(fm) = frontmatter {
            if fm.trim().is_empty() {
                body.to_string()
            } else {
                format!("---\n{}\n---\n\n{}", fm.trim_end(), body)
            }
        } else {
            body.to_string()
        };

        crate::tools::write::grug_write(
            self,
            &category,
            &stem,
            &file_content,
            Some(&brain.name),
            None,
        )?;

        let new_mtime = read_mtime(self, &brain.name, &canonical_rel);
        let v = json!({"path": canonical_rel, "etag": new_mtime});
        serde_json::to_string(&v).map_err(|e| e.to_string())
    }

    fn memory_delete_json(
        &mut self,
        brain_name: &str,
        rel_path: &str,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let brain = self.resolve_brain(Some(brain_name))?.clone();

        if !brain.writable {
            let v = json!({"error": "read-only brain", "brain": brain.name});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let (category, stem) = split_rel_path(rel_path)?;
        validate_memory_path(&category)?;
        validate_memory_path(&stem)?;

        crate::tools::delete::grug_delete(self, &category, &stem, Some(brain_name), false)
            .map(|_| serde_json::to_string(&json!({"ok": true})).unwrap())
    }

    fn memory_rename_json(
        &mut self,
        brain_name: &str,
        old_rel_path: &str,
        new_rel_path: &str,
        rewrite_links: bool,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let brain = self.resolve_brain(Some(brain_name))?.clone();

        if !brain.writable {
            let v = json!({"error": "read-only brain", "brain": brain.name});
            return serde_json::to_string(&v).map_err(|e| e.to_string());
        }

        let (old_cat, old_stem) = split_rel_path(old_rel_path)?;
        let (new_cat, new_stem) = split_rel_path(new_rel_path)?;
        validate_memory_path(&old_cat)?;
        validate_memory_path(&old_stem)?;
        validate_memory_path(&new_cat)?;
        validate_memory_path(&new_stem)?;

        let result = crate::tools::rename::grug_rename_with_links(
            self,
            &old_cat,
            &old_stem,
            &new_cat,
            &new_stem,
            Some(brain_name),
            rewrite_links,
        );

        match result {
            Err(e) => {
                let (err_kind, msg) = if e.contains("source not found") {
                    ("not found", e.clone())
                } else if e.contains("destination already exists") {
                    ("destination exists", e.clone())
                } else if e.contains("read-only") {
                    ("read-only brain", e.clone())
                } else {
                    ("error", e.clone())
                };
                let v = json!({"error": err_kind, "message": msg});
                serde_json::to_string(&v).map_err(|e| e.to_string())
            }
            Ok((new_canonical, affected)) => {
                let new_mtime = read_mtime(self, &brain.name, &new_canonical);
                let v = json!({
                    "path": new_canonical,
                    "etag": new_mtime,
                    "affected_paths": affected,
                });
                serde_json::to_string(&v).map_err(|e| e.to_string())
            }
        }
    }
}
