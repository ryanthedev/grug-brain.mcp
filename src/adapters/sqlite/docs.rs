//! `impl DocsPort for GrugDb` — read-only `grug-docs` browsing.

use crate::domain::ports::DocsPort;
use crate::tools::GrugDb;

impl DocsPort for GrugDb {
    fn grug_docs(
        &mut self,
        category: Option<&str>,
        path: Option<&str>,
        page: Option<usize>,
    ) -> Result<String, String> {
        crate::tools::docs::grug_docs(self, category, path, page)
    }
}
