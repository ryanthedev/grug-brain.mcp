//! `impl SearchPort for GrugDb` — FTS5 + quickswitch search.
//!
//! `grug_search` wraps the existing `tools::search::grug_search` (which
//! returns `String` with no error path) into the `Result<String, String>`
//! contract. The other two delegate straight through.

use crate::domain::ports::SearchPort;
use crate::tools::GrugDb;

impl SearchPort for GrugDb {
    fn grug_search(
        &mut self,
        query: &str,
        page: Option<usize>,
    ) -> Result<String, String> {
        // tools::search::grug_search returns a plain `String`; lift it.
        Ok(crate::tools::search::grug_search(self, query, page))
    }

    fn search_json(
        &mut self,
        query: &str,
        brain: Option<&str>,
    ) -> Result<String, String> {
        crate::http::handlers::search_json(self, query, brain)
    }

    fn quickswitch_json(&mut self, query: &str) -> Result<String, String> {
        crate::http::handlers::quickswitch_json(self, query)
    }
}
