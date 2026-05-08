//! `impl BrainPort for GrugDb` — system-level brain queries.

use crate::db::SCHEMA_VERSION;
use crate::domain::ports::BrainPort;
use crate::tools::GrugDb;
use serde_json::{json, Value};

impl BrainPort for GrugDb {
    fn brains_json(&mut self) -> Result<String, String> {
        self.maybe_reload_config();
        let arr: Vec<Value> = self
            .config()
            .brains
            .iter()
            .map(|b| {
                json!({
                    "name": b.name,
                    "primary": b.primary,
                    "writable": b.writable,
                    "source": b.source,
                    "flat": b.flat,
                })
            })
            .collect();
        serde_json::to_string(&Value::Array(arr)).map_err(|e| e.to_string())
    }

    fn healthz_json(&mut self) -> Result<String, String> {
        self.maybe_reload_config();
        let last_index_at: Option<f64> = self
            .conn()
            .query_row("SELECT MAX(mtime) FROM files", [], |row| row.get(0))
            .ok();
        let brains: Vec<Value> = self
            .config()
            .brains
            .iter()
            .map(|b| {
                let last: Option<f64> = self
                    .conn()
                    .query_row(
                        "SELECT MAX(mtime) FROM files WHERE brain = ?1",
                        [&b.name],
                        |row| row.get(0),
                    )
                    .ok();
                json!({"name": b.name, "last_sync_at": last})
            })
            .collect();
        let v = json!({
            "ok": true,
            "schema_version": SCHEMA_VERSION,
            "last_index_at": last_index_at,
            "brains": brains,
        });
        serde_json::to_string(&v).map_err(|e| e.to_string())
    }
}
