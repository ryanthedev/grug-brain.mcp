//! `impl WritePort for GrugDb` — every persisted-state mutation.
//!
//! Methods split between two legacy homes:
//!   - MCP-shaped (`grug_write`/`grug_delete`/`grug_update`) live in
//!     `crate::tools::*` and produce markdown text.
//!   - HTTP-shaped (`memory_*_json`) live in `crate::http::handlers` and
//!     produce JSON strings.
//!
//! Phase 3/4 eventually consolidates these; Phase 2 just unifies the entry
//! points behind one trait.

use crate::domain::ports::WritePort;
use crate::tools::update::EditEntry;
use crate::tools::GrugDb;

impl WritePort for GrugDb {
    fn grug_write(
        &mut self,
        category: &str,
        path: &str,
        content: &str,
        brain: Option<&str>,
        if_match_mtime: Option<f64>,
    ) -> Result<String, String> {
        crate::tools::write::grug_write(self, category, path, content, brain, if_match_mtime)
    }

    fn grug_delete(
        &mut self,
        category: &str,
        path: &str,
        brain: Option<&str>,
        hard: bool,
    ) -> Result<String, String> {
        crate::tools::delete::grug_delete(self, category, path, brain, hard)
    }

    fn grug_update(
        &mut self,
        category: &str,
        path: &str,
        edits: &[EditEntry],
        brain: Option<&str>,
    ) -> Result<String, String> {
        crate::tools::update::grug_update(self, category, path, edits, brain)
    }

    fn memory_write_json(
        &mut self,
        brain: &str,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
        if_match_etag: f64,
        attempted_body: &str,
    ) -> Result<String, String> {
        crate::http::handlers::memory_write_json(
            self,
            brain,
            rel_path,
            body,
            frontmatter,
            if_match_etag,
            attempted_body,
        )
    }

    fn memory_create_json(
        &mut self,
        brain: Option<&str>,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
    ) -> Result<String, String> {
        crate::http::handlers::memory_create_json(self, brain, rel_path, body, frontmatter)
    }

    fn memory_delete_json(
        &mut self,
        brain: &str,
        rel_path: &str,
    ) -> Result<String, String> {
        crate::http::handlers::memory_delete_json(self, brain, rel_path)
    }

    fn memory_rename_json(
        &mut self,
        brain: &str,
        old_rel_path: &str,
        new_rel_path: &str,
        rewrite_links: bool,
    ) -> Result<String, String> {
        crate::http::handlers::memory_rename_json(
            self,
            brain,
            old_rel_path,
            new_rel_path,
            rewrite_links,
        )
    }
}
