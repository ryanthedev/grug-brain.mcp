//! `impl GraphPort for GrugDb` — cross-link similarity graph queries.

use crate::domain::ports::GraphPort;
use crate::tools::GrugDb;

impl GraphPort for GrugDb {
    fn graph_json(
        &mut self,
        brain: Option<&str>,
        mode: Option<&str>,
        node: Option<&str>,
        depth: Option<usize>,
    ) -> Result<String, String> {
        crate::http::handlers::graph_json(self, brain, mode, node, depth)
    }

    fn graph_local_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
        hops: u64,
    ) -> Result<String, String> {
        crate::http::handlers::graph_local_json(self, brain, path, hops)
    }
}
