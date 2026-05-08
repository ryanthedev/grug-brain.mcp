//! Domain ports — every operation grug-brain exposes lives here as a trait
//! method. This file is the readable map of the system: scan it to know
//! every MCP tool and HTTP endpoint without reading any implementation.
//!
//! ## Conventions
//!
//! - One trait per concern (10 total, 24 methods covering all dispatch
//!   arms in `server.rs::dispatch_tool`).
//! - All methods return `Result<String, String>` to match the existing
//!   error convention (`String` is markdown text for MCP arms, JSON for
//!   `__http/*` arms — the eventual Phase 2 implementations preserve the
//!   current shapes byte-for-byte).
//! - Every method takes `&mut self` because every current implementation
//!   calls `db.maybe_reload_config()` first; uniform mutability avoids
//!   Phase 2 borrow-checker friction.
//! - Each method's doc comment names the MCP tool (`grug-*`) and/or HTTP
//!   dispatch arm (`__http/*`) it serves — that link is what makes this
//!   file the system's index.

use crate::tools::update::EditEntry;

/// Brain-level system queries: what brains exist, are they healthy.
///
/// Serves the read-only "system info" pane in the web UI and grug status
/// checks. No MCP tools live here — `grug-recall` covers brain listing on
/// the MCP side via `RecallPort::grug_read`.
pub trait BrainPort {
    /// Lists every configured brain as JSON.
    ///
    /// Serves: `__http/brains` (HTTP dispatch arm).
    fn brains_json(&mut self) -> Result<String, String>;

    /// Reports server health: index freshness, per-brain last sync time,
    /// brain count.
    ///
    /// Serves: `__http/healthz` (HTTP dispatch arm).
    fn healthz_json(&mut self) -> Result<String, String>;
}

/// Per-memory queries — list, fetch one, list tags, list backlinks. These
/// are the HTTP "read a memory and its metadata" surface. The MCP read path
/// is `RecallPort::grug_read`.
pub trait MemoryPort {
    /// Lists memories in a brain (or all brains) as JSON metadata rows.
    ///
    /// Serves: `__http/memories` (HTTP dispatch arm).
    fn memories_json(&mut self, brain: Option<&str>) -> Result<String, String>;

    /// Fetches a single memory's full content + frontmatter as JSON.
    ///
    /// Serves: `__http/memory` (HTTP dispatch arm).
    fn memory_json(
        &mut self,
        brain: &str,
        category: &str,
        path: &str,
    ) -> Result<String, String>;

    /// Lists every tag in a brain (or all brains) with occurrence counts.
    ///
    /// Serves: `__http/tags` (HTTP dispatch arm).
    fn tags_json(&mut self, brain: Option<&str>) -> Result<String, String>;

    /// Lists memories that link to the given memory.
    ///
    /// Serves: `__http/backlinks` (HTTP dispatch arm).
    fn backlinks_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
    ) -> Result<String, String>;
}

/// Full-text and quickswitch search across memories. Both transports use
/// FTS5 BM25 ranking under the hood; the difference is response shape
/// (paginated markdown for MCP, JSON for HTTP).
pub trait SearchPort {
    /// FTS5 query with BM25 ranking, returns paginated markdown text.
    ///
    /// Serves: `grug-search` (MCP tool).
    fn grug_search(
        &mut self,
        query: &str,
        page: Option<usize>,
    ) -> Result<String, String>;

    /// FTS5 query with optional brain filter, returns JSON hits.
    ///
    /// Serves: `__http/search` (HTTP dispatch arm).
    fn search_json(
        &mut self,
        query: &str,
        brain: Option<&str>,
    ) -> Result<String, String>;

    /// Name-prefix LIKE match across all brains for the command palette.
    ///
    /// Serves: `__http/quickswitch` (HTTP dispatch arm).
    fn quickswitch_json(&mut self, query: &str) -> Result<String, String>;
}

/// Cross-link / cosine similarity graph queries. The full-graph view
/// powers the global graph visualization; the local view scopes to a
/// single memory's neighborhood.
pub trait GraphPort {
    /// Full cross-link cosine-similarity graph for one or all brains.
    ///
    /// Serves: `__http/graph` (HTTP dispatch arm).
    fn graph_json(
        &mut self,
        brain: Option<&str>,
        mode: Option<&str>,
        node: Option<&str>,
        depth: Option<usize>,
    ) -> Result<String, String>;

    /// Local graph: BFS up to `hops` edges from a focus memory.
    ///
    /// Serves: `__http/graph_local` (HTTP dispatch arm).
    fn graph_local_json(
        &mut self,
        brain: Option<&str>,
        path: &str,
        hops: u64,
    ) -> Result<String, String>;
}

/// Memory mutations — create, update, delete, rename. Both MCP-style
/// (`grug-write` etc., taking `category` + `path`) and HTTP-style
/// (`__http/memory_*`, taking `rel_path` + `frontmatter` separately) live
/// here because they all change persisted memory state.
pub trait WritePort {
    /// Writes a memory file. Frontmatter is auto-generated from
    /// `category`/`path`. Optimistic concurrency via `if_match_mtime`.
    ///
    /// Serves: `grug-write` (MCP tool).
    fn grug_write(
        &mut self,
        category: &str,
        path: &str,
        content: &str,
        brain: Option<&str>,
        if_match_mtime: Option<f64>,
    ) -> Result<String, String>;

    /// Soft-deletes a memory (or hard-deletes when `hard = true`).
    ///
    /// Serves: `grug-delete` (MCP tool).
    fn grug_delete(
        &mut self,
        category: &str,
        path: &str,
        brain: Option<&str>,
        hard: bool,
    ) -> Result<String, String>;

    /// Applies an ordered list of `old → new` edit substitutions to a
    /// memory's body.
    ///
    /// Serves: `grug-update` (MCP tool).
    fn grug_update(
        &mut self,
        category: &str,
        path: &str,
        edits: &[EditEntry],
        brain: Option<&str>,
    ) -> Result<String, String>;

    /// Writes a memory by raw `rel_path` + body + optional frontmatter
    /// JSON, with ETag-based optimistic concurrency. Used by the web
    /// editor.
    ///
    /// Serves: `__http/memory_write` (HTTP dispatch arm).
    fn memory_write_json(
        &mut self,
        brain: &str,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
        if_match_etag: f64,
        attempted_body: &str,
    ) -> Result<String, String>;

    /// Creates a new memory at `rel_path`, failing if it already exists.
    ///
    /// Serves: `__http/memory_create` (HTTP dispatch arm).
    fn memory_create_json(
        &mut self,
        brain: Option<&str>,
        rel_path: &str,
        body: &str,
        frontmatter: Option<&str>,
    ) -> Result<String, String>;

    /// Deletes a memory by raw `rel_path` (HTTP-style, no soft-delete
    /// semantics — the file is removed).
    ///
    /// Serves: `__http/memory_delete` (HTTP dispatch arm).
    fn memory_delete_json(
        &mut self,
        brain: &str,
        rel_path: &str,
    ) -> Result<String, String>;

    /// Renames a memory and (optionally) rewrites every wiki-link that
    /// pointed at the old path.
    ///
    /// Serves: `__http/memory_rename` (HTTP dispatch arm).
    fn memory_rename_json(
        &mut self,
        brain: &str,
        old_rel_path: &str,
        new_rel_path: &str,
        rewrite_links: bool,
    ) -> Result<String, String>;
}

/// "What memories exist" entry points — the MCP-side counterparts to
/// `MemoryPort`. `grug-recall` is the brain/category index browse;
/// `grug-read` is the bottom-out read of a single memory file.
pub trait RecallPort {
    /// Lists recent memories grouped by brain/category as markdown.
    ///
    /// Serves: `grug-recall` (MCP tool).
    fn grug_recall(
        &mut self,
        category: Option<&str>,
        brain: Option<&str>,
    ) -> Result<String, String>;

    /// Reads a memory or browses brains/categories depending on which
    /// arguments are supplied (no args → list brains; brain only →
    /// list categories; brain + category → list memories; all three →
    /// read file).
    ///
    /// Serves: `grug-read` (MCP tool).
    fn grug_read(
        &mut self,
        brain: Option<&str>,
        category: Option<&str>,
        path: Option<&str>,
    ) -> Result<String, String>;
}

/// Periodic maintenance pass over all brains: stale-memory triage,
/// cross-link suggestions, missing-frontmatter detection. Single
/// long-running operation.
pub trait DreamPort {
    /// Runs the maintenance/dream pass and returns the markdown report.
    ///
    /// Serves: `grug-dream` (MCP tool).
    fn grug_dream(&mut self) -> Result<String, String>;
}

/// Git sync orchestration — pull/push for git-backed brains.
pub trait SyncPort {
    /// Syncs one brain (or all brains) with their git remote.
    ///
    /// Serves: `grug-sync` (MCP tool).
    fn grug_sync(&mut self, brain: Option<&str>) -> Result<String, String>;
}

/// Read-only doc browsing — surfaces the docs brain (vendored
/// markdown documentation) without exposing it to the writable
/// memory tools.
pub trait DocsPort {
    /// Lists docs categories or reads a docs page (paginated).
    ///
    /// Serves: `grug-docs` (MCP tool).
    fn grug_docs(
        &mut self,
        category: Option<&str>,
        path: Option<&str>,
        page: Option<usize>,
    ) -> Result<String, String>;
}

/// Brain configuration management: add/remove/edit/list brains in
/// `brains.json`. Long parameter list mirrors the MCP tool schema —
/// each `Option` corresponds to a JSON field clients may set.
pub trait ConfigPort {
    /// Mutates or queries `brains.json` based on `action`
    /// (`list`, `add`, `remove`, `set`, `primary`, `make-primary`).
    ///
    /// Serves: `grug-config` (MCP tool).
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
    ) -> Result<String, String>;
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    /// The source of `ports.rs` — read at compile time so DW assertions
    /// can grep its contents without filesystem I/O at test time.
    const PORTS_SRC: &str = include_str!("ports.rs");

    /// Every dispatch arm name that must appear as a `Serves:` line in
    /// some method's doc comment. Cross-checked against
    /// `server.rs::dispatch_tool` arms.
    const ALL_DISPATCH_ARMS: &[&str] = &[
        // MCP arms (10)
        "grug-search",
        "grug-write",
        "grug-read",
        "grug-recall",
        "grug-delete",
        "grug-config",
        "grug-sync",
        "grug-dream",
        "grug-update",
        "grug-docs",
        // HTTP arms (14)
        "__http/brains",
        "__http/memories",
        "__http/memory",
        "__http/graph",
        "__http/search",
        "__http/quickswitch",
        "__http/healthz",
        "__http/tags",
        "__http/backlinks",
        "__http/graph_local",
        "__http/memory_write",
        "__http/memory_create",
        "__http/memory_delete",
        "__http/memory_rename",
    ];

    /// Returns every `pub trait Foo` name declared in `ports.rs`.
    fn declared_traits() -> Vec<String> {
        let mut traits = Vec::new();
        for line in PORTS_SRC.lines() {
            let line = line.trim_start();
            if let Some(rest) = line.strip_prefix("pub trait ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    traits.push(name);
                }
            }
        }
        traits
    }

    /// Returns every `fn foo` name declared at trait scope (anywhere
    /// `fn name(` appears with no preceding `pub`/`impl`/`//`).
    fn declared_methods() -> Vec<String> {
        let mut methods = Vec::new();
        let mut in_test_mod = false;
        for line in PORTS_SRC.lines() {
            // Skip the test module body — those `fn`s are helpers, not
            // trait methods.
            if line.trim_start().starts_with("mod tests") {
                in_test_mod = true;
            }
            if in_test_mod {
                continue;
            }
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("fn ") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    methods.push(name);
                }
            }
        }
        methods
    }

    /// For every method, walk back through preceding `///` doc lines
    /// (skipping blank/attr lines that sit between docs and `fn`) and
    /// return the concatenated comment text. Returns a map of
    /// method-name → joined-doc-text.
    fn method_doc_blocks() -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        let lines: Vec<&str> = PORTS_SRC.lines().collect();
        let mut in_test_mod = false;
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("mod tests") {
                in_test_mod = true;
            }
            if in_test_mod {
                continue;
            }
            let trimmed = line.trim_start();
            let Some(rest) = trimmed.strip_prefix("fn ") else {
                continue;
            };
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            // Walk back collecting `///` lines, skipping `#[...]` attrs
            // and blank lines. Stop on anything else.
            let mut j = i;
            let mut docs = Vec::new();
            while j > 0 {
                j -= 1;
                let prev = lines[j].trim_start();
                if prev.is_empty() || prev.starts_with("#[") {
                    continue;
                }
                if let Some(d) = prev.strip_prefix("///") {
                    docs.push(d.trim().to_string());
                } else {
                    break;
                }
            }
            docs.reverse();
            out.insert(name, docs.join(" "));
        }
        out
    }

    #[test]
    fn test_DW_1_1_has_at_least_ten_traits() {
        let traits = declared_traits();
        assert!(
            traits.len() >= 10,
            "DW-1.1: expected ≥10 trait definitions, found {}: {:?}",
            traits.len(),
            traits
        );
    }

    #[test]
    fn test_DW_1_1_covers_all_24_dispatch_methods() {
        // Every dispatch arm name (with `-`/`__http/` punctuation
        // stripped) should map to one method name in `ports.rs`.
        // Mapping rule: take the arm tail and replace non-ident chars
        // with `_`. `__http/foo` → `foo_json` (existing convention);
        // `grug-foo` → `grug_foo`.
        let methods = declared_methods();
        let mut missing = Vec::new();
        for arm in ALL_DISPATCH_ARMS {
            let expected = arm_to_method(arm);
            if !methods.iter().any(|m| m == &expected) {
                missing.push((*arm, expected));
            }
        }
        assert!(
            missing.is_empty(),
            "DW-1.1: dispatch arms with no matching trait method: {:?}\n\
             declared methods: {:?}",
            missing,
            methods
        );
        assert_eq!(
            methods.len(),
            ALL_DISPATCH_ARMS.len(),
            "DW-1.1: trait method count ({}) must equal dispatch arm count ({})",
            methods.len(),
            ALL_DISPATCH_ARMS.len()
        );
    }

    #[test]
    fn test_DW_1_2_every_method_has_dispatch_arm_in_doc() {
        let docs = method_doc_blocks();
        let methods = declared_methods();
        let mut undocumented = Vec::new();
        for m in &methods {
            let doc = docs.get(m).cloned().unwrap_or_default();
            // Each method must mention either an MCP arm (`grug-…`) or
            // an HTTP arm (`__http/…`) inside its doc block.
            let has_mcp = doc.contains("grug-");
            let has_http = doc.contains("__http/");
            if !has_mcp && !has_http {
                undocumented.push(m.clone());
            }
        }
        assert!(
            undocumented.is_empty(),
            "DW-1.2: methods with no MCP/HTTP arm referenced in doc comment: {:?}",
            undocumented
        );
    }

    /// Map a dispatch arm name to its expected trait-method name.
    /// `grug-foo` → `grug_foo`; `__http/foo` → `foo_json`.
    fn arm_to_method(arm: &str) -> String {
        if let Some(tail) = arm.strip_prefix("__http/") {
            format!("{tail}_json")
        } else {
            arm.replace('-', "_")
        }
    }
}
