# Discovery: Phase 1 - Project Scaffold + Core Types

## Files Found
- `server.js` (1715 lines) -- the entire current implementation
- `docs/plans/2026-04-01-rust-redesign.md` -- the redesign plan
- `~/.grug-brain/brains.json` -- live config with 8 brains (2 writable, 6 flat/read-only)
- `~/.grug-brain/grug.db` -- live database at schema version 5

## Current State
- No `src/` directory exists -- greenfield Rust project
- No `Cargo.toml` exists
- No `docs/code-standards.md` exists
- Rust 1.91.1 installed via Homebrew
- rmcp crate v1.3.0 available on crates.io
- Existing grug.db confirmed at schema version 5 with tables: meta, files, brain_fts (FTS5), dream_log, cross_links

## Gaps
- None. Plan assumptions match reality.
- Schema version 5 confirmed in live DB
- FTS5 tokenizer config: `porter unicode61` confirmed
- brains.json has `source` field (for flat brains) not mentioned in plan -- irrelevant to Phase 1 (no git/sync), but should be preserved in the Brain type

## Prerequisites
- [x] Rust toolchain installed (1.91.1)
- [x] rmcp crate available (1.3.0)
- [x] Live brains.json and grug.db available for reference
- [x] server.js available for porting reference

## Assumption Verification
- **rusqlite bundled FTS5 matches Bun's SQLite FTS5 behavior**: CONFIRMED. Both use standard SQLite FTS5 with `porter unicode61` tokenizer. rusqlite's `bundled` feature compiles SQLite from source with FTS5 enabled. The schema is standard SQL -- no Bun-specific extensions.

## Recommendation
BUILD -- everything needed is available, no blockers.
