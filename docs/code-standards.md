<!-- base-commit: 8d5378f -->
<!-- generated: 2026-05-06 -->

# Code Standards — grug-brain

## Architecture

Single-binary Rust MCP server + in-process axum HTTP server + vanilla-JS web viewer.
- `src/main.rs` → `src/client.rs` (MCP tool definitions) → `src/server.rs` (dispatch loop, owns the DB thread).
- All tool logic in `src/tools/*.rs`, one file per tool. Shared state via `GrugDb`.
- `src/http/` runs the read-only HTTP/SSE API beside the MCP socket. Handlers send `__http/*` requests through `db_tx` to the DB thread; never touch SQLite directly.
- `web/` is vendored vanilla JS (no build step). sigma.js/graphology/CodeMirror 6/DOMPurify/marked/jsdiff are checked into `web/vendor/`.

Data flow: markdown files → `walker.rs` → `indexing.rs` → SQLite FTS5 → tools query → returned as formatted strings or JSON. Watcher (`src/watcher.rs`) notifies the server, which broadcasts to SSE subscribers.

## Naming

- Tool files: `src/tools/{verb}.rs` (e.g., `search.rs`, `dream.rs`, `write.rs`)
- Public functions: `grug_{tool}(db: &mut GrugDb, ...)` pattern
- HTTP handlers: short verb names in `src/http/handlers.rs` (`brains`, `memories`, `preview`, `healthz`)
- DB-thread routes: `__http/{name}` matching the handler
- Test helpers: `src/tools/mod.rs::test_helpers`
- Frontend modules: lowercase namespaces inside the outer IIFE (`api`, `state`, `render`, `router`, `sse`, `graph`, `toast`, `theme`, `editor`, `save`, `conflict`, `crud`, `autocomplete`, `palette`, etc.) — each is `const x = (() => { ... return { api }; })();`
- Design tokens: `--font-body: "Helvetica Neue", Helvetica, Arial, sans-serif`. Spacing on 4px grid (4·8·12·16·24·32·40·48px). Active states use amber `--accent-warm` left-border rule, not background flood. Type scale: 11/13/16/20/25px (≥25% jumps between levels).

## Imports

- `use super::GrugDb` in tool files for the shared db wrapper
- `use crate::types::*` for shared types
- `rusqlite::params!` macro for parameterized queries
- HTTP handlers go through `call_db(&state.db_tx, "__http/route", payload)` — no direct SQLite access
- Frontend: no module bundler. Everything is one IIFE in `web/app.js`. Vendor libs loaded via `<script>` in `web/index.html`.

## Error Handling

**Rust tools:** return `Result<String, String>` — Ok = user-facing message, Err = failure. "Not found" → `Ok("not found: ...")` not Err. Read-only brain → `Ok("brain \"...\" is read-only")`. DB errors → `.map_err(|e| format!("context: {e}"))`.

**HTTP handlers:** return `Result<Json<Value>, ApiError>`. `ApiError::internal`/`bad_request`/`not_found` produce structured JSON. Path traversal etc. validated via `helpers::validate_memory_path`.

**Frontend:** every `api.*` call returns `{ok, data, error}` — never throws. Render layer checks `ok` and routes failures to `toast.error()`. User-controlled data goes through `escapeHtml()` or `textContent`; markdown body goes through `DOMPurify.sanitize()` before any innerHTML assignment.

## File Organization

```
src/
  main.rs / client.rs / server.rs / config.rs / db.rs / parsing.rs
  helpers.rs       — path validation, frontmatter assembly
  walker.rs        — filesystem walking
  watcher.rs       — notify-rs file watcher → broadcast channel
  types.rs         — shared structs
  tools/
    mod.rs         — GrugDb + test_helpers
    {tool}.rs
  http/
    mod.rs         — AppState, router, listen
    handlers.rs    — axum handlers + DB-thread JSON producers
    security.rs    — Host/CORS/CSRF/CSP middleware
    sse.rs         — SSE channel
    assets.rs      — rust-embed for web/ + content-hash
web/
  index.html / app.js / styles.css
  vendor/          — cytoscape, dompurify, marked (vendored, not npm)
tests/
  http_integration.rs / socket_integration.rs
  playwright/      — Playwright suite (one spec per DW item)
```

## Testing

**Rust:** every tool file has `#[cfg(test)] mod tests`. Use `test_helpers::test_db()` (single brain) or `test_db_multi()` (multi). `create_brain_file()` writes fixtures. Tests use `tempfile::TempDir`. Pattern: create file → index → exercise → assert string.

**HTTP integration:** `tests/http_integration.rs` spins up the full axum server against a temp brain. Reuse this harness for new endpoints; don't mock the DB layer.

**Playwright:** one test per Done-When item, named `dw-N.M: …`. `tests/playwright/fixtures.js` provides the `grugServer` fixture (boots a release binary against a fixture brain on a free port, sets `GRUG_DB`). Use `await expect(locator).toHaveAttribute("aria-pressed", "true")` etc. — prefer a11y selectors over CSS. Run with `make test-playwright`.

**Property tests:** use `proptest` for write-path invariants (path validation, frontmatter round-trip, ETag conflict resolution). See `tests/property_write.rs`.

## Technology Decisions

- SQLite FTS5 + porter stemming + BM25; WAL mode; schema versioned in `meta` (current: 8) — drop-and-recreate on mismatch.
- `rmcp` for MCP transport; UDS socket beside the HTTP listener.
- `axum` 0.7 with tower middleware; Host/CORS/CSRF/CSP enforced on every request.
- `notify` for the file watcher → `tokio::sync::broadcast` for fan-out → SSE.
- `rust-embed` ships `web/` into the binary; FNV-1a content-hash for cache-busting (`?v=hash`).
- Frontend has no build step. Vendor JS libs live in `web/vendor/` and are committed. Adding a dep = vendoring a minified file + size note in the PR.
- sigma.js + graphology replace cytoscape for the graph view. `web/vendor/sigma.min.js`, `web/vendor/graphology.min.js`.
- CodeMirror 6: vendored as a single rolled-up bundle via `web/build/`. Don't introduce npm at the frontend root.

## Forbidden Patterns

- **No direct SQLite access from HTTP handlers** — always go through `db_tx` + `__http/*` routes. The DB thread is single-writer; bypassing it races.
- **No raw `innerHTML` from user data.** Use `textContent` or `escapeHtml()`. Markdown body MUST go through DOMPurify with the existing allowlist.
- **No `.unwrap()` on user paths.** Always `map_err` or handle gracefully. `validate_memory_path` for anything that touches the filesystem.
- **No external network calls** from tool code or handlers. Everything is local.
- **No npm/bundler at the frontend root.** Vendor or write it yourself. Playwright's `tests/playwright/package.json` is the only frontend npm tree.
- **No new web/ files without DW coverage.** Every new UI surface needs at least one `dw-N.M` Playwright spec + axe-core check.

## Similar Implementations

- HTTP handler + DB route pair: `src/http/handlers.rs::preview` ↔ `preview_json` (DB-thread side) — copy this shape for new endpoints.
- Write-path with ETag: `src/tools/write.rs` (Plan 1 Phase 1 hardening; mtime-based ETag, conflict returns Err).
- Watcher → SSE fan-out: `src/watcher.rs` + `src/http/sse.rs`.
- Frontend pub-sub: `web/app.js` `state` IIFE + `state.subscribe(render)`.
- Graph render: `web/app.js` `graph.*` namespace — `renderGraph` is `async` (yields via rAF+setTimeout before heavy work). Only renders nodes with at least one edge; falls back to all nodes when no edges exist. Layout: category radial for >50 nodes, Fruchterman-Reingold for ≤50.
- Graph edge SQL: `cross_links` and `links` queries in `graph_json` branch on `brain_owned` — `WHERE brain_a = ?1 AND brain_b = ?1` when brain is Some, unfiltered when None. Use sequential `?N` param ordering (rusqlite binds positionally).
- Cross-links: `dream.rs` — cosine similarity (TF-IDF) cross-link insertion into `cross_links` table.
- Search: `search.rs` — FTS5 + BM25 + pagination.
