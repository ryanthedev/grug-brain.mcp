<!-- base-commit: d3af7c1 -->
<!-- generated: 2026-04-16 -->

# Code Standards — grug-brain

## Architecture

Single-binary Rust MCP server. Entry point: `src/main.rs` → `src/client.rs` (MCP tool definitions) → `src/server.rs` (dispatch). All tool logic lives in `src/tools/*.rs` — one file per tool. Shared state via `GrugDb` (SQLite connection + `BrainConfig`).

Data flow: markdown files on disk → `indexing.rs` syncs to SQLite FTS5 → tools query FTS/files tables → results returned as formatted strings.

## Naming

- Tool files: `src/tools/{verb}.rs` (e.g., `search.rs`, `dream.rs`, `update.rs`)
- Public functions: `grug_{tool}(db: &mut GrugDb, ...)` pattern
- Internal helpers: private `fn` in same file, no cross-tool imports except via `mod.rs` re-exports
- Types: `src/types.rs` — shared structs (`Brain`, `BrainConfig`, `FtsRow`, `SearchResult`, `RecallRow`, `Memory`)
- Test helpers: `src/tools/mod.rs::test_helpers` module

## Imports

- `use super::GrugDb` in tool files for the shared db wrapper
- `use crate::types::*` for shared types
- `rusqlite::params!` macro for parameterized queries
- No external HTTP/network dependencies — everything is local SQLite

## Error Handling

- Tool functions return `Result<String, String>` — Ok for user-facing messages, Err for failures
- "Not found" conditions return `Ok("not found: ...")` not `Err` (matching user-facing convention)
- Read-only brain → `Ok("brain \"...\" is read-only")`
- Database errors → `.map_err(|e| format!("context: {e}"))`

## File Organization

```
src/
  main.rs          — CLI + MCP server bootstrap
  client.rs        — #[tool] MCP method definitions + param structs
  server.rs        — JSON-RPC dispatch
  config.rs        — brains.json loading
  db.rs            — SQLite schema init (version 6)
  parsing.rs       — frontmatter/body extraction
  types.rs         — shared structs
  walker.rs        — filesystem walking
  tools/
    mod.rs         — GrugDb struct + test_helpers
    {tool}.rs      — one per MCP tool
```

## Testing

- Every tool file has `#[cfg(test)] mod tests` at bottom
- `test_helpers::test_db()` creates temp dir + in-memory-like SQLite
- `test_helpers::test_db_multi()` for multi-brain tests
- `create_brain_file()` helper for writing test fixtures
- Tests use `tempfile::TempDir` for isolation
- Pattern: create brain file → index → exercise tool → assert output string

## Technology Decisions

- SQLite FTS5 with porter stemming for full-text search (schema version 6)
- BM25 ranking (FTS5 built-in) for search relevance
- WAL journal mode for concurrent reads
- Schema versioned in `meta` table -- drop-and-recreate on version mismatch (current: version 6)
- No ORM — raw SQL via rusqlite
- `schemars` for JSON Schema generation (MCP param validation)
- `rmcp` crate for MCP transport

## Forbidden Patterns

- No direct filesystem access to brain dirs from tool code — always go through indexing.rs
- No `.unwrap()` on user-facing paths — always map_err or handle gracefully
- No external network calls from tool functions

## Similar Implementations

- Cross-links: `dream.rs:123-216` — FTS-based keyword matching across memories
- Search: `search.rs` — FTS5 queries with BM25 ranking, pagination
- Indexing: `indexing.rs` — disk → SQLite sync with mtime diffing
