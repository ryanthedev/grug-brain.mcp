//! `impl SyncPort for GrugDb` — `grug-sync` git pull/push.

use crate::domain::ports::SyncPort;
use crate::tools::GrugDb;

impl SyncPort for GrugDb {
    fn grug_sync(&mut self, brain: Option<&str>) -> Result<String, String> {
        crate::tools::sync::grug_sync(self, brain)
    }
}
