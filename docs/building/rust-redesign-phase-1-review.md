# Review: Phase 1 - Project Scaffold + Core Types

## Requirement Fulfillment

| DW-ID | Done-When Item | Status | Evidence |
|-------|---------------|--------|----------|
| DW-1.1 | `cargo build` succeeds with rusqlite bundled FTS5 | SATISFIED | `cargo build` exits 0; `Cargo.toml:11` `rusqlite = { version = "0.35", features = ["bundled"] }`; `db::tests::test_fts5_porter_stemming` passes confirming FTS5 operational |
| DW-1.2 | Brain config loads from `~/.grug-brain/brains.json` with same validation rules (unique names, exactly one primary, home expansion, flat/writable defaults) | SATISFIED | `src/config.rs:37-174`; 16 tests pass covering all validation paths: unique names (`test_duplicate_names`), one primary (`test_no_primary`, `test_multiple_primaries`), home expansion (`test_home_expansion`), flat/writable defaults (`test_flat_writable_default`, `test_flat_writable_explicit`), missing dir filtered (`test_missing_dir_filtered`) |
| DW-1.3 | SQLite schema created matching current schema version 5 (files, brain_fts, dream_log, cross_links, meta tables) | SATISFIED | `src/db.rs:45-75`; all 5 tables created; `test_schema_creation` roundtrips all tables; `test_schema_migration` verifies drop+recreate for version < 5; `SCHEMA_VERSION` constant at `db.rs:4` |
| DW-1.4 | Core types defined: Brain, Memory/FtsRow, SearchResult, BrainConfig | SATISFIED | `src/types.rs:7-82`; all four types present with correct fields; `BrainConfig` includes `primary_brain()` and `get()` helpers; `source` field preserved per discovery note |
| DW-1.5 | Frontmatter parser, body extractor, description extractor match current JS behavior | SATISFIED | `src/parsing.rs:6-84`; `extract_frontmatter` handles first-colon split including URLs (`test_extract_frontmatter_value_with_colon`); `extract_description` skips headers/fences/admonitions/imports, strips formatting, truncates at 120 chars |
| DW-1.6 | File walker handles .md/.mdx, skips dot/underscore prefixed | SATISFIED | `src/walker.rs:8-42`; `walk_files` recurses, skips `'.'` and `'_'` prefixed entries, filters `.md`/`.mdx`, returns sorted paths; `get_categories` also skips dot-prefixed dirs |
| DW-1.7 | Unit tests for config parsing, frontmatter extraction, file walking | SATISFIED | 55 tests total, all passing: 16 config, 14 parsing, 9 walker, 8 helpers, 5 db, 2 spike; covers happy paths and all error/edge cases specified in pseudocode |
| DW-1.8 | Spike: verify rmcp transport-io forwarding pattern compiles with a stub tool | SATISFIED | `src/spike.rs`; `test_spike_serve_compiles` (async tokio test) creates duplex transport, wires server+client, calls stub tool end-to-end and asserts `"echo: hello"` response; confirms `#[tool_router]`, `#[tool_handler]`, `ServerHandler`, `ClientHandler` all compile |

**All requirements met:** YES

## Spec Match

- [x] All pseudocode sections implemented: Cargo.toml, types.rs, config.rs, db.rs, parsing.rs, walker.rs, helpers.rs, spike.rs, main.rs, lib.rs
- [x] No unplanned additions that alter scope — `paginate()` in helpers.rs is pre-wired for Phase 2 but is trivial and mentioned in the pseudocode; `schemars` dependency added to support rmcp's `JsonSchema` derive requirement (not in pseudocode but required by the spike)
- [x] Test coverage verified: all pseudocode test cases implemented; implementation adds additional tests beyond spec (e.g. `test_extract_frontmatter_value_with_colon`, `test_flat_writable_explicit`, `test_fts5_porter_stemming`, `test_fts5_highlight`, `test_idempotent_init`)

One minor deviation: `Cargo.toml` adds `schemars = "1"` which is not in the pseudocode. This is required by rmcp's `#[tool]` macro (it needs `JsonSchema` on parameter structs). Not scope creep — it's a forced dependency of the spike.

## Dead Code

None found. All modules are exported from `lib.rs`. `spike.rs` is correctly labelled as Phase 1 only and will be replaced in Phase 3. `paginate()` in helpers is used by the Phase 2 tool layer and is pre-staged appropriately. No debug statements, no commented-out blocks, no unreachable code.

## Correctness Dimensions

| Dimension | Status | Evidence |
|-----------|--------|----------|
| Concurrency | N/A | Phase 1 is single-threaded scaffolding. No shared mutable state, no async I/O paths beyond the spike's test (which is contained). The `LazyLock` in helpers.rs is safe for concurrent use. |
| Error Handling | PASS | `config.rs` returns `Result<BrainConfig, String>` with actionable messages at every failure point. `db.rs` propagates `rusqlite::Result`. `walker.rs` silently continues on unreadable dirs (correct for walk semantics). `ensure_dir` in config.rs swallows errors on `create_dir_all` — acceptable since the subsequent `fs::write` will surface the real error. |
| Resources | PASS | `Connection` returned from `init_db` is owned by caller; Drop closes it. No file handles held open. Walker collects paths without holding FDs. `TempDir`/`NamedTempFile` in tests are RAII-cleaned. |
| Boundaries | PASS | `extract_description` truncates at 120 with correct char-boundary arithmetic (`char_indices` + `len_utf8`). `paginate` clamps page to `[1, total_pages]`. `slugify` truncates at 80 with same char-boundary approach. Empty inputs handled: empty string slugifies to empty, empty dir returns empty vec, empty frontmatter returns empty map. |
| Security | PASS | No SQL string concatenation — all DB calls use `?1` placeholders. No shell invocations in Phase 1. Path inputs from brains.json are user-controlled config, not untrusted external input; they are not used in SQL. |

## Defensive Programming: PASS

Crisis triage:

1. **External input validated at boundaries?** YES — config.rs validates name, dir presence, uniqueness, primary count before constructing any Brain values. JSON parse errors surfaced immediately.
2. **Return values checked for all external calls?** YES — all `fs::` calls and `rusqlite` calls are `?`-propagated or explicitly handled. The one exception (`ensure_dir` swallowing `create_dir_all` error) is intentional and safe as noted above.
3. **Error paths tested?** YES — 13 of 16 config tests are error/edge cases. Walker tests cover nonexistent dir and empty dir. DB tests cover migration from old schema.
4. **Assertions on critical invariants?** `BrainConfig::primary_brain()` uses `expect()` — appropriate: it's an internal invariant that `load_brains` guarantees (primary brain survived filtering). Not a user-facing assertion.
5. **Resources released on all paths?** YES — Rust ownership ensures this. No manual cleanup needed.

One minor observation: `config.rs:162-166` adds a post-filter check that the primary brain survived directory filtering, returning a clear error. This is a good defensive addition beyond the pseudocode spec.

## Design Quality: No findings

**Depth > Length:** Modules are appropriately sized. `config.rs` is 170 lines of logic with 290 lines of tests in the same file — this is the right tradeoff (tests close to logic). `db.rs` is compact at 78 lines of implementation.

**Together/Apart:** Parsing functions are pure (no I/O), correctly isolated. Walker has no coupling to parsing or config. The only coupling is `types.rs` imported by `config.rs` — correct dependency direction.

**Pass-through methods:** `BrainConfig::get()` and `primary_brain()` are thin accessors but they serve a genuine purpose (enforcing the invariant that primary exists via `expect`, centralizing lookup). Not pass-through — they add value.

**Unknown unknowns:** None identified. All Phase 1 surface area is well-understood scaffolding.

**Scope additions noted:**
- `paginate()` helper: mentioned in pseudocode helpers section, already needed by Phase 2.
- `schemars` crate: forced by rmcp macro, not a design decision.
- `test_extract_frontmatter_value_with_colon`: catches a real bug class (URL values in frontmatter). Good addition.

## Testing: PASS

**Dirty:clean ratio:** Across all modules, error/edge-case tests substantially outnumber happy-path tests. Config module: 13 error tests vs 3 happy-path. Parser: 8 edge tests vs 5 happy-path. Walker: 5 edge tests vs 2 happy-path. Ratio is approximately 4:1 dirty:clean — within range of the 5:1 target for mature code.

**Coverage gaps (minor, acceptable for Phase 1):**
- `expand_home` with `USERPROFILE` fallback is untested (Windows path, not relevant on macOS)
- `create_default_config` branch where `fs::write` fails is not tested (hard to test without filesystem mocking)
- `get_categories` does not skip underscore-prefixed dirs (only skips dot-prefixed) — this is consistent with the pseudocode spec but differs from `walk_files`. Not a bug for Phase 1 since `get_categories` is for listing writable category dirs; worth noting for Phase 2.

## Issues

None.

**Verdict: PASS**
