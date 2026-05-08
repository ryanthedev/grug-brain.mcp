//! `impl BrainPort for GrugDb` — system-level brain queries.
//!
//! Methods delegate to the legacy DB-thread `*_json` helpers in
//! `crate::http::handlers`. Phase 3 will move those bodies in here and
//! delete the originals; Phase 2 keeps both alive for a no-behavior-change
//! refactor.

use crate::domain::ports::BrainPort;
use crate::tools::GrugDb;

impl BrainPort for GrugDb {
    fn brains_json(&mut self) -> Result<String, String> {
        crate::http::handlers::brains_json(self)
    }

    fn healthz_json(&mut self) -> Result<String, String> {
        crate::http::handlers::healthz_json(self)
    }
}
