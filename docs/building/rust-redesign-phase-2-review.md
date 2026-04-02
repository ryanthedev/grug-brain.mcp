# Review: Phase 2 - Tool Implementations

## Requirement Fulfillment

| DW-ID | Done-When Item | Status | Evidence |
|-------|---------------|--------|----------|
| DW-2.1 | All 9 tools implemented as functions returning structured results | SATISFIED | `src/tools/search.rs:grug_search`, `write.rs:grug_write`, `read.rs:grug_read`, `recall.rs:grug_recall`, `delete.rs:grug_delete`, `config.rs:grug_config`, `sync.rs:grug_sync`, `dream.rs:grug_dream`, `docs.rs:grug_docs` — all 9 present as public functions |
| DW-2.2 | FTS5 search with BM25 ranking, porter stemming, highlight snippets | SATISFIED | `src/tools/search.rs:57-66` — SQL uses `highlight(brain_fts, 5, '>>>', '<<<')`, `ORDER BY rank` (BM25), porter stemming is schema-level from Phase 1; `test_fts_search_bm25_ranking` and `test_fts_search_highlight_snippets` pass |
| DW-2.3 | File indexing (walk + parse + insert) matches current behavior | SATISFIED | `src/tools/indexing.rs:index_file` (line 11), `sync_brain` (line 104); `src/walker.rs` fixed to skip underscore-prefixed dirs; `test_sync_brain_basic`, `test_index_file_basic` pass |
| DW-2.4 | Config hot-reload (mtime-based lazy check) works | SATISFIED | `src/tools/mod.rs:GrugDb::maybe_reload_config` (line 50); compares mtime as `Option<f64>`, reloads on change, called at top of every tool; `test_config_add` exercises the forced-reload path |
| DW-2.5 | Dream tool: cross-links, stale detection (90 days), quality issues, conflict listing, dream log marking | SATISFIED | `src/tools/dream.rs:grug_dream` — conflicts at line 37, needs-review query at line 84, cross-link upsert at line 182, stale filter at line 219 (STALE_DAYS=90), quality issues at line 265, dream_log upsert at line 346; tests: `test_dream_stale_detection`, `test_dream_quality_issues`, `test_dream_conflict_listing`, `test_dream_marks_reviewed` all pass |
| DW-2.6 | Recall tool: 2-most-recent-per-category preview, writes full listing to recall.md | SATISFIED | `src/tools/recall.rs:grug_recall` — full listing written at line 57, preview limited to 2 at line 64, "... and N more" at line 72; always writes to `primary_brain.dir/recall.md` matching JS; `test_recall_preview_limits_to_2` and `test_recall_writes_recall_md` pass |
| DW-2.7 | Unit tests for each tool against a temp SQLite database | SATISFIED | 121 total tests pass; each tool file has a `#[cfg(test)]` module using `test_db()` backed by `TempDir` with real SQLite — search (9 tests), indexing (4 tests), write (6 tests), read (9 tests), recall (5 tests), delete (5 tests), config (9 tests), sync (4 tests), dream (6 tests), docs (4 tests) |

**All requirements met:** YES

---

## Spec Match

- [x] `src/types.rs` — `refresh_interval: Option<u64>` added to Brain; `ToolResult` not added as a struct (tools return `Result<String, String>` instead — see deviation note)
- [x] `src/walker.rs` — `get_categories` now skips underscore-prefixed dirs (line 61)
- [x] `src/tools/mod.rs` — `GrugDb`, `maybe_reload_config`, `resolve_brain`, constants, `test_helpers` all present
- [x] `src/tools/search.rs` — `build_fts_query`, `fts_search`, `search_all`, `grug_search` present
- [x] `src/tools/indexing.rs` — `index_file`, `remove_file`, `sync_brain` present
- [x] `src/tools/write.rs` — `grug_write` present
- [x] `src/tools/read.rs` — `grug_read` with all 6 cases implemented
- [x] `src/tools/recall.rs` — `grug_recall` with preview and recall.md write present
- [x] `src/tools/delete.rs` — `grug_delete` present
- [x] `src/tools/config.rs` — `grug_config` with list/add/remove present
- [x] `src/tools/sync.rs` — `grug_sync` present
- [x] `src/tools/dream.rs` — `grug_dream` with all 6 sections present
- [x] `src/tools/docs.rs` — `grug_docs` (deprecated) present
- [x] `src/lib.rs` — `pub mod tools;` added

**Deviations:**

1. `ToolResult` struct not added to `types.rs`. The pseudocode specified `pub struct ToolResult { pub text: String }` as a uniform return type. The implementation uses `Result<String, String>` directly. This is a minor deviation — the semantic is equivalent and `Result<String, String>` is arguably cleaner for Rust (separates user-facing content from internal errors). The deviation is low risk because Phase 3 will convert these return values to MCP format regardless.

2. No unplanned additions detected.

**Test coverage:** 121 tests, all 10 tool/utility files covered. Matches plan's "unit tests per tool against temp SQLite" requirement.

---

## Dead Code

None found. All imported modules are used. No unreachable code blocks detected. Git-stub comments (`// Git commit skipped (Phase 4)`) are intentional annotations, not dead code.

One observation: `src/spike.rs` is declared in `lib.rs` but is a Phase 1 artifact that is not used anywhere. It is not a new problem and is out of scope for this phase. Low severity.

---

## Correctness Dimensions

| Dimension | Status | Evidence |
|-----------|--------|----------|
| Concurrency | N/A | Phase 2 is single-threaded. `GrugDb` holds `Connection` with `&mut self` on all tools, preventing concurrent access. Thread safety is explicitly deferred to Phase 3. |
| Error Handling | PASS | DB errors propagate via `Result`; user-visible errors return `Ok(String)` (matching JS behavior of returning error text as tool output); no panics in tool paths; rusqlite query failures use `map_err` or fall through to empty-vec. One silent swallow: `index_file` failure in `sync_brain` (line 134) is silently ignored via `if ... .is_ok()` — this matches JS behavior where indexing errors were also non-fatal. |
| Resources | PASS | No file handles left open; `fs::read_to_string` and `fs::write` are self-closing; temp dirs in tests use `TempDir` RAII. |
| Boundaries | PASS | Page numbers: `max(1, page)` applied consistently. Empty FTS query returns early before hitting SQLite. Path components validated (brain name regex in config add). Flat vs category brain path resolution handled in read (line 62) and sync (line 120). |
| Security | PASS | Brain name validated against `^[a-z0-9][a-z0-9-]*$` before writing to config. SQL uses parameterized queries throughout — no string interpolation into SQL. File writes are restricted to brain directories. |

---

## Defensive Programming: PASS

No silent failures that would hide real errors:

- `maybe_reload_config` swallows parse errors on reload (`Err(_) => {}` at line 71 of mod.rs) — this matches the stated JS behavior ("keep current brains on parse error") and is documented in a comment. Acceptable.
- `sync_brain` silently ignores failed `index_file` calls (matching JS non-fatal behavior). This is consistent but means indexing errors go unreported. Low risk for Phase 2 (no server yet).
- `get_cross_links` in `read.rs` returns empty vec on DB error rather than propagating — appropriate since cross-links are optional decoration.
- All tool functions validate preconditions (brain writable, brain exists) before performing mutations.
- No broad `catch`/`unwrap` patterns in tool logic; `unwrap()` only appears in test helpers where panics are acceptable.

---

## Design Quality: PASS with minor findings

**Depth vs length:** GrugDb is appropriately deep — it encapsulates connection + config + hot-reload in a clean interface. Tool functions are long (dream.rs is ~350 lines) but handle inherently complex logic; no artificial splitting needed.

**Code duplication (LOW):** `recall_all` and `recall_by_category` are duplicated verbatim between `recall.rs` and `dream.rs`. These are private helpers in separate modules. In Phase 3 or 4, extracting them to a shared query module would reduce maintenance burden, but it is not a blocker.

**Unknown unknowns: none found.** The JS reference logic is faithfully ported and the test coverage catches the key behavioral cases.

**Pass-through methods:** None — `GrugDb` accessors (`conn()`, `config()`, `config_mut()`) are genuinely needed indirection rather than trivial pass-throughs.

---

## Output Formatting: FAIL on unicode divergences

This is the constraint from the dispatch: "All 9 tools must produce output structurally identical to current JS (same field names, same pagination behavior)."

Three places where Rust output differs from JS output:

**1. Dream — em-dash `—` vs double hyphen `--` (dream.rs:111 and server.js:1452)**

JS: `"all clean — nothing needs review"` (U+2014 em-dash)
Rust: `"all clean -- nothing needs review"` (two hyphens)

JS: `"nothing to dream about — no memories yet"` (em-dash)
Rust: `"nothing to dream about -- no memories yet"` (two hyphens)

**2. Dream — arrow character `↔` vs `<->` (dream.rs:209 and server.js:1495)**

JS: `"- {a} ↔ {b}"` (U+2194 left-right arrow)
Rust: `"- {a} <-> {b}"` (ASCII arrow)

**3. Recall — ellipsis `…` vs `...` (recall.rs:73 and server.js:1125)**

JS: `"  … and {n} more"` (U+2026 horizontal ellipsis)
Rust: `"  ... and {n} more"` (three dots)

These are low-stakes cosmetic differences (Claude Code sees the text output, not raw bytes) but they violate the "structurally identical" constraint stated in the plan. They would fail a byte-for-byte comparison.

**Verdict for this dimension: FAIL — 3 unicode output divergences from JS reference.**

---

## Testing: PASS

- 121 tests, 0 failures
- Every tool has dedicated tests against a real temp SQLite database
- Key behaviors tested: create/update distinction, read-only rejection, stale detection, dream_log mark-reviewed, cross-link upsert, recall.md write, pagination header, "and N more" preview truncation, config round-trips (add/remove/duplicate), sync idempotency
- The `test_fts_search_highlight_snippets` test has a comment noting ambiguity about which FTS column `highlight(brain_fts, 5, ...)` targets — the assertion falls back to a weak check. Not a blocker but the comment reveals uncertainty about column indexing.

---

## Issues

1. **Unicode divergences from JS output (3 instances)**
   - Files:
     - `src/tools/dream.rs:111` — `"all clean -- nothing needs review"` should be `"all clean — nothing needs review"`
     - `src/tools/dream.rs:25` — `"nothing to dream about -- no memories yet"` should use em-dash
     - `src/tools/dream.rs:209` — `"<->"` should be `"↔"`
     - `src/tools/recall.rs:73` — `"..."` should be `"…"`
   - Fix: Replace ASCII approximations with the exact Unicode characters used in server.js

---

## Self-Check

- [x] All 7 DW items from the dispatch prompt are in the table (count verified: DW-2.1 through DW-2.7)
- [x] No DW items omitted
- [x] Every SATISFIED item has file:line evidence
- [x] Verdict follows the rules: the 3 unicode divergences are a real finding against the explicit "structurally identical" constraint, making this a FAIL per that constraint

**Verdict: FAIL — 3 unicode characters in tool output text diverge from JS reference (em-dash, ellipsis, left-right arrow). All other requirements are fully met. Fix is mechanical: substitute the correct Unicode literals at the 4 call sites listed above.**
