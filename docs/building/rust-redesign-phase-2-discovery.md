# Discovery: Phase 2 - Tool Implementations

## Files Found

### Existing Rust source (Phase 1 output)
- `src/lib.rs` -- module declarations (config, db, helpers, parsing, spike, types, walker)
- `src/types.rs` -- Brain, BrainConfig, FtsRow, SearchResult, Memory structs
- `src/db.rs` -- init_db with schema v5 (files, brain_fts, dream_log, cross_links, meta)
- `src/config.rs` -- load_brains, load_brains_from, expand_home, config validation
- `src/parsing.rs` -- extract_frontmatter, extract_body, extract_description
- `src/walker.rs` -- walk_files, get_categories
- `src/helpers.rs` -- slugify, today, paginate (PAGE_SIZE=50)
- `src/main.rs` -- stub (prints version)
- `src/spike.rs` -- rmcp transport-io spike
- `Cargo.toml` -- rusqlite bundled, chrono, clap, regex, rmcp, serde, tokio, tempfile

### JS reference implementation
- `server.js` lines 567-621: prepared statements (all SQL queries)
- `server.js` lines 639-656: indexFile / removeFile
- `server.js` lines 836-863: buildFtsQuery / ftsSearch / searchAll
- `server.js` lines 886-924: grug-write
- `server.js` lines 926-958: grug-search
- `server.js` lines 960-1078: grug-read (complex backwards-compat)
- `server.js` lines 1080-1130: grug-recall
- `server.js` lines 1132-1159: grug-delete
- `server.js` lines 1160-1323: grug-config (list/add/remove)
- `server.js` lines 1325-1352: grug-sync
- `server.js` lines 1354-1549: grug-dream (cross-links, stale, quality, conflicts, dream log)
- `server.js` lines 1551-1625: grug-docs (deprecated)
- `index-worker.js` lines 62-128: sync worker (walk, diff, index/remove)

## Current State

Phase 1 is complete with 55 passing tests. All foundational types, config loading, parsing, walking, and database schema are in place. No tool implementation code exists yet.

## Gaps

1. **Missing `refresh_interval` on Brain struct** -- the grug-config add/remove logic references `refreshInterval` on brain entries but the Rust Brain struct has no `refresh_interval` field. Needed for config round-trips.

2. **`get_categories()` skips dot-prefixed but not underscore-prefixed dirs** -- inconsistent with `walk_files` which skips both. Noted in the dispatch prompt as a Phase 2 fix.

3. **No `src/tools/` module** -- needs to be created.

4. **Constants SEARCH_PAGE_SIZE=20 and BROWSE_PAGE_SIZE=100 not defined** -- helpers.rs has PAGE_SIZE=50 for text pagination but not the search/browse page sizes.

5. **FTS query building not implemented** -- buildFtsQuery logic needs to be ported.

6. **No sync_brain function** -- the sync tool calls syncBrainAsync which uses a worker. For Phase 2, we implement the sync logic inline (single-threaded walk+diff+index/remove).

## Prerequisites
- [x] Cargo project builds with rusqlite bundled FTS5
- [x] Brain config loading works
- [x] SQLite schema v5 created (all 5 tables)
- [x] Core types defined (Brain, BrainConfig, FtsRow, SearchResult, Memory)
- [x] Frontmatter/body/description extraction
- [x] File walker
- [x] Helpers (slugify, today, paginate)
- [x] 55 tests passing

## Recommendation
BUILD -- all prerequisites met. Proceed with tool implementations.
