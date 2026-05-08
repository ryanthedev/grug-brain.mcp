//! `impl MemoryPort for GrugDb` — per-memory HTTP read endpoints.
//!
//! Thin delegators to `crate::http::handlers::*_json`. Phase 3 absorbs
//! the bodies and removes the standalone functions.

use crate::domain::ports::MemoryPort;
use crate::tools::GrugDb;

impl MemoryPort for GrugDb {
    fn memories_json(&mut self, brain: Option<&str>) -> Result<String, String> {
        crate::http::handlers::memories_json(self, brain)
    }

    fn memory_json(
        &mut self,
        brain: &str,
        category: &str,
        path: &str,
    ) -> Result<String, String> {
        crate::http::handlers::memory_json(self, brain, category, path)
    }

    fn tags_json(&mut self, brain: Option<&str>) -> Result<String, String> {
        crate::http::handlers::tags_json(self, brain)
    }

    fn backlinks_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
    ) -> Result<String, String> {
        crate::http::handlers::backlinks_json(self, brain, path)
    }
}
