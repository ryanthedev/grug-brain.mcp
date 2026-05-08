//! `impl ConfigPort for GrugDb` — `grug-config` MCP tool dispatch.

use crate::domain::ports::ConfigPort;
use crate::tools::GrugDb;

impl ConfigPort for GrugDb {
    #[allow(clippy::too_many_arguments)]
    fn grug_config(
        &mut self,
        action: &str,
        name: Option<&str>,
        dir: Option<&str>,
        primary: Option<bool>,
        writable: Option<bool>,
        flat: Option<bool>,
        git: Option<&str>,
        sync_interval: Option<u64>,
        source: Option<&str>,
        refresh_interval: Option<u64>,
    ) -> Result<String, String> {
        crate::tools::config::grug_config(
            self,
            action,
            name,
            dir,
            primary,
            writable,
            flat,
            git,
            sync_interval,
            source,
            refresh_interval,
        )
    }
}
