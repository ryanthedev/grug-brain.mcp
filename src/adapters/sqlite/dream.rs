//! `impl DreamPort for GrugDb` — periodic maintenance pass.

use crate::domain::ports::DreamPort;
use crate::tools::GrugDb;

impl DreamPort for GrugDb {
    fn grug_dream(&mut self) -> Result<String, String> {
        crate::tools::dream::grug_dream(self)
    }
}
