//! `impl MemoryPort for GrugDb` — per-memory HTTP read endpoints.

use crate::domain::ports::MemoryPort;
use crate::helpers::validate_memory_path;
use crate::parsing;
use crate::tools::similarity::find_similar;
use crate::tools::GrugDb;
use serde_json::{json, Value};

impl MemoryPort for GrugDb {
    fn memories_json(&mut self, brain: Option<&str>) -> Result<String, String> {
        self.maybe_reload_config();
        let target = match brain {
            Some(name) => Some(self.resolve_brain(Some(name))?.name.clone()),
            None => None,
        };

        let (sql, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match &target {
            Some(name) => (
                "SELECT path, brain, category, name, description, date FROM brain_fts WHERE brain = ?1 ORDER BY category, date DESC",
                vec![name as &dyn rusqlite::types::ToSql],
            ),
            None => (
                "SELECT path, brain, category, name, description, date FROM brain_fts ORDER BY brain, category, date DESC",
                vec![],
            ),
        };

        let mut stmt = self.conn().prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                let path: String = row.get(0)?;
                let brain: String = row.get(1)?;
                let category: String = row.get(2)?;
                let name: String = row.get(3)?;
                let description: String = row.get(4)?;
                let date: String = row.get(5)?;
                Ok((path, brain, category, name, description, date))
            })
            .map_err(|e| e.to_string())?;

        let mut out: Vec<Value> = Vec::new();
        for r in rows {
            let (path, brain, category, name, description, date) =
                r.map_err(|e| e.to_string())?;
            // mtime from files table
            let mtime: f64 = self
                .conn()
                .query_row(
                    "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
                    rusqlite::params![&brain, &path],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);
            // Tags for this memory (Phase 6: drives the tag-pane filter).
            let tags: Vec<String> = self
                .conn()
                .prepare("SELECT tag FROM tags WHERE brain = ?1 AND path = ?2 ORDER BY tag")
                .ok()
                .and_then(|mut s| {
                    s.query_map(rusqlite::params![&brain, &path], |row| {
                        row.get::<_, String>(0)
                    })
                    .ok()
                    .map(|it| it.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default();
            out.push(json!({
                "path": path,
                "brain": brain,
                "category": category,
                "name": name,
                "description": description,
                "date": date,
                "mtime": mtime,
                "tags": tags,
            }));
        }
        serde_json::to_string(&Value::Array(out)).map_err(|e| e.to_string())
    }

    fn memory_json(
        &mut self,
        brain: &str,
        category: &str,
        path: &str,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        validate_memory_path(category)?;
        validate_memory_path(path)?;

        let brain_obj = self.resolve_brain(Some(brain))?.clone();
        let file_name = if path.ends_with(".md") {
            path.to_string()
        } else {
            format!("{path}.md")
        };
        let rel_path = if brain_obj.flat {
            file_name.clone()
        } else {
            format!("{category}/{file_name}")
        };
        let abs_path = brain_obj.dir.join(&rel_path);
        if !abs_path.exists() {
            return Ok(
                serde_json::to_string(&json!({"not_found": true})).unwrap()
            );
        }

        let content = std::fs::read_to_string(&abs_path)
            .map_err(|e| format!("read {}: {e}", abs_path.display()))?;
        let frontmatter = parsing::extract_frontmatter(&content);
        let body = parsing::extract_body(&content);
        let mtime: f64 = self
            .conn()
            .query_row(
                "SELECT mtime FROM files WHERE brain = ?1 AND path = ?2",
                rusqlite::params![&brain_obj.name, &rel_path],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let neighbors = find_similar(self.conn(), &brain_obj.name, &rel_path, 10)
            .unwrap_or_default()
            .into_iter()
            .map(|s| json!({"path": s.path, "brain": s.brain, "score": s.score}))
            .collect::<Vec<_>>();

        let v = json!({
            "frontmatter": frontmatter,
            "body": body,
            "mtime": mtime,
            "neighbors": neighbors,
        });
        serde_json::to_string(&v).map_err(|e| e.to_string())
    }

    fn tags_json(&mut self, brain: Option<&str>) -> Result<String, String> {
        self.maybe_reload_config();
        let brain_owned: Option<String> = match brain {
            Some(name) => Some(self.resolve_brain(Some(name))?.name.clone()),
            None => None,
        };
        let (sql, params): (&str, Vec<&dyn rusqlite::types::ToSql>) = match &brain_owned {
            Some(name) => (
                "SELECT tag, COUNT(*) AS c FROM tags WHERE brain = ?1 \
                 GROUP BY tag ORDER BY c DESC, tag ASC",
                vec![name as &dyn rusqlite::types::ToSql],
            ),
            None => (
                "SELECT tag, COUNT(*) AS c FROM tags \
                 GROUP BY tag ORDER BY c DESC, tag ASC",
                vec![],
            ),
        };
        let mut stmt = self.conn().prepare(sql).map_err(|e| e.to_string())?;
        let rows: Vec<Value> = stmt
            .query_map(params.as_slice(), |row| {
                Ok(json!({
                    "tag": row.get::<_, String>(0)?,
                    "count": row.get::<_, i64>(1)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        serde_json::to_string(&Value::Array(rows)).map_err(|e| e.to_string())
    }

    fn backlinks_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let target_brain = self.resolve_brain(brain)?.name.clone();
        let mut stmt = self
            .conn()
            .prepare(
                "SELECT l.brain, l.src_path, f.category, f.name \
                 FROM links l \
                 JOIN brain_fts f ON f.brain = l.brain AND f.path = l.src_path \
                 WHERE l.target_brain = ?1 AND l.target_path = ?2 \
                 ORDER BY l.brain, f.category, f.name",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<Value> = stmt
            .query_map(rusqlite::params![&target_brain, path], |row| {
                Ok(json!({
                    "brain":    row.get::<_, String>(0)?,
                    "path":     row.get::<_, String>(1)?,
                    "category": row.get::<_, String>(2)?,
                    "name":     row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        serde_json::to_string(&Value::Array(rows)).map_err(|e| e.to_string())
    }
}
