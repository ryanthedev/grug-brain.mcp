//! `impl RecallPort for GrugDb` — `grug-recall` and `grug-read` MCP tools.

use crate::domain::ports::RecallPort;
use crate::tools::GrugDb;

impl RecallPort for GrugDb {
    fn grug_recall(
        &mut self,
        category: Option<&str>,
        brain: Option<&str>,
    ) -> Result<String, String> {
        crate::tools::recall::grug_recall(self, category, brain)
    }

    fn grug_read(
        &mut self,
        brain: Option<&str>,
        category: Option<&str>,
        path: Option<&str>,
    ) -> Result<String, String> {
        crate::tools::read::grug_read(self, brain, category, path)
    }
}
