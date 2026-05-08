//! `impl SearchPort for GrugDb` — FTS5 + quickswitch search.

use crate::domain::ports::SearchPort;
use crate::tools::search::search_all;
use crate::tools::GrugDb;
use serde_json::{json, Value};

impl SearchPort for GrugDb {
    fn grug_search(
        &mut self,
        query: &str,
        page: Option<usize>,
    ) -> Result<String, String> {
        Ok(crate::tools::search::grug_search(self, query, page))
    }

    fn search_json(
        &mut self,
        query: &str,
        brain: Option<&str>,
    ) -> Result<String, String> {
        self.maybe_reload_config();
        let (results, total) = search_all(self.conn(), query, None);
        let filtered: Vec<&_> = match brain {
            Some(b) => results.iter().filter(|r| r.brain == b).collect(),
            None => results.iter().collect(),
        };
        let hits: Vec<Value> = filtered
            .iter()
            .map(|r| {
                json!({
                    "path": r.path,
                    "brain": r.brain,
                    "category": r.category,
                    "name": r.name,
                    "date": r.date,
                    "description": r.description,
                    "snippet": r.snippet,
                    "rank": r.rank,
                })
            })
            .collect();
        serde_json::to_string(&json!({"hits": hits, "total": total}))
            .map_err(|e| e.to_string())
    }

    fn quickswitch_json(&mut self, query: &str) -> Result<String, String> {
        self.maybe_reload_config();
        let pattern = format!("%{}%", query.to_lowercase());
        let mut stmt = self
            .conn()
            .prepare(
                "SELECT path, brain, category, name FROM brain_fts \
                 WHERE LOWER(name) LIKE ?1 ORDER BY name LIMIT 50",
            )
            .map_err(|e| e.to_string())?;
        let hits: Vec<Value> = stmt
            .query_map([&pattern], |row| {
                Ok(json!({
                    "path": row.get::<_, String>(0)?,
                    "brain": row.get::<_, String>(1)?,
                    "category": row.get::<_, String>(2)?,
                    "name": row.get::<_, String>(3)?,
                }))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        serde_json::to_string(&json!({"hits": hits})).map_err(|e| e.to_string())
    }
}
