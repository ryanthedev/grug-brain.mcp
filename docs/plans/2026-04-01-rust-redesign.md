# Plan: Rust Redesign

**Created:** 2026-04-01
**Status:** ready
**Complexity:** complex

---

## Context

Redesign grug-brain.mcp as a single Rust binary replacing the current JS/Bun monolith (1715-line `server.js`). Two modes: `grug serve` (brew service, Unix socket listener, owns all state) and `grug --stdio` (MCP stdio thin client for Claude Code). Motivation is performance — especially CLI startup latency — and distribution as a single `brew install` binary with no runtime dependencies.

## Constraints

- Single Rust binary, installable via `brew install`
- Server required — CLI fails cleanly if server is not running
- Unix domain socket for all CLI-to-server communication (no HTTP)
- SQLite FTS5 retained, schema-compatible with existing grug.db (schema version 5)
- Full git sync logic preserved: pull/rebase/push, conflict resolution, .grugignore, sync:false
- All 9 MCP tools with feature parity
- Must handle concurrent requests from multiple Claude Code sessions
- Claude Code plugin updated to use installed Rust binary

## Chosen Approach

**Single Rust binary, server-first architecture**

Server (`grug serve`) owns all state: SQLite database, git sync, file indexing, brain timers. MCP stdio mode (`grug --stdio`) is a thin client that forwards every tool call to the running server over a Unix domain socket. This gives near-zero CLI startup time (~1ms) since the CLI never opens SQLite or touches disk beyond connecting to the socket.

**Rationale:** User prioritizes CLI speed above all else. Server-required is acceptable — the brew service ensures it's always running. Single writer to SQLite eliminates all concurrency concerns. Unix socket is the fastest local IPC (~2-5us latency).

**Fallback:** If rmcp's transport abstraction doesn't support the forwarding pattern cleanly, implement MCP tool dispatch manually with a match on tool name — the tool set is fixed and small (9 tools).

## Rejected Approaches

- **Embedded-first (both modes open SQLite):** Slower CLI startup (~5-10ms), concurrent SQLite access complexity, two code paths to maintain.
- **Server-first + standalone fallback:** Best of both worlds but doubles code paths for every operation. Complexity not justified.
- **Go server + Rust CLI (two binaries):** User chose single binary in Rust for simplicity and distribution.
- **HTTP API on server:** Unnecessary overhead. Unix socket is faster and sufficient for local-only communication.

---

## Implementation Phases

### Phase 1: Project Scaffold + Core Types
**Model:** opus
**Skills:** code-foundations:code

**Goal:** Set up the Rust project structure, define core types, and get rusqlite + FTS5 working with schema-compatible database.

**Scope:**
- IN: Cargo project setup, dependencies (rusqlite bundled, tokio, rmcp, clap, serde), brain config parsing from `~/.grug-brain/brains.json`, SQLite schema creation matching current schema version 5, core data types (Brain, Memory, SearchResult, FtsRow), frontmatter/body parsing, file walker, slugify/date helpers
- OUT: No tool implementations, no transport, no git operations

**Constraints:**
- Existing `~/.grug-brain/grug.db` must be usable — same tables, same columns, same FTS5 tokenizer config

**Approach notes:**
- Migration strategy: schema version < 5 → drop and recreate (matching current JS behavior, no incremental migration)

**File hints:**
- `src/` — new Rust source tree
- `Cargo.toml` — project manifest
- Current `server.js` lines 28-118 (brain config), 515-620 (schema + statements)

**Depends on:** None | **Unlocks:** Phase 2

**Done when:**
- [ ] DW-1.1: `cargo build` succeeds with rusqlite bundled FTS5
- [ ] DW-1.2: Brain config loads from `~/.grug-brain/brains.json` with same validation rules as current JS (unique names, exactly one primary, home expansion, flat/writable defaults)
- [ ] DW-1.3: SQLite schema created matching current schema version 5 (files, brain_fts, dream_log, cross_links, meta tables)
- [ ] DW-1.4: Core types defined: Brain, Memory/FtsRow, SearchResult, BrainConfig
- [ ] DW-1.5: Frontmatter parser, body extractor, description extractor match current JS behavior
- [ ] DW-1.6: File walker (walkFiles equivalent) handles .md/.mdx, skips dot/underscore prefixed
- [ ] DW-1.7: Unit tests for config parsing, frontmatter extraction, file walking
- [ ] DW-1.8: Spike: verify rmcp transport-io forwarding pattern compiles with a stub tool (de-risk Phase 3)

**Difficulty:** LOW
**Uncertainty:** None

---

### Phase 2: Tool Implementations
**Model:** opus
**Skills:** code-foundations:code

**Goal:** Implement all 9 MCP tool handlers as functions that operate on the shared database — direct port of current JS logic.

**Scope:**
- IN: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-config, grug-sync, grug-dream, grug-docs. File indexing (index/remove). FTS5 query building. Pagination. Config read/write mutation. Dream logic (cross-links, stale detection, quality issues, conflict detection, dream log).
- OUT: No transport layer, no git operations (sync/dream commit stubs), no background services

**Constraints:**
- All 9 tools must produce output structurally identical to current JS (same field names, same pagination behavior)
- No schema migrations — reuse existing grug.db

**Approach notes:**
- Port logic directly from server.js lines 886-1627
- grug-dream cross-link logic: delete existing links, search by name terms, sort/dedup by stable primary key, upsert
- grug-config add/remove: mutate brains.json on disk, force reload
- grug-read backwards-compat: category-without-brain searches primary first then all brains
- FTS query building: single term = `"term"*`, multiple = OR joined with `"term"*`

**File hints:**
- `src/tools/` — one module per tool or grouped
- Current `server.js` lines 836-1627 (all tool handlers)

**Depends on:** Phase 1 | **Unlocks:** Phase 3

**Done when:**
- [ ] DW-2.1: All 9 tools implemented as functions returning structured results
- [ ] DW-2.2: FTS5 search with BM25 ranking, porter stemming, highlight snippets
- [ ] DW-2.3: File indexing (walk + parse + insert) matches current behavior
- [ ] DW-2.4: Config hot-reload (mtime-based lazy check) works
- [ ] DW-2.5: Dream tool: cross-links, stale detection (90 days), quality issues, conflict listing, dream log marking
- [ ] DW-2.6: Recall tool: 2-most-recent-per-category preview, writes full listing to recall.md
- [ ] DW-2.7: Unit tests for each tool against a temp SQLite database

**Difficulty:** MEDIUM
**Uncertainty:** None — direct port of existing, well-understood logic

---

### Phase 3: Unix Socket Server + MCP Stdio Client
**Model:** opus
**Skills:** code-foundations:code

**Goal:** Wire up the two runtime modes: `grug serve` listens on a Unix domain socket dispatching tool calls, `grug --stdio` bridges MCP stdio protocol to the socket.

**Scope:**
- IN: Tokio Unix socket server, request/response protocol (newline-delimited JSON over socket), rmcp MCP stdio transport, clap CLI argument parsing (`serve`, `--stdio`), concurrent connection handling, socket path convention (`~/.grug-brain/grug.sock`), PID file for server lifecycle
- OUT: No git sync, no background timers, no brew/plugin integration

**Constraints:**
- Transport must be Unix domain socket only — no HTTP or TCP
- Must handle concurrent requests from multiple Claude sessions without blocking

**Approach notes:**
- Protocol format: newline-delimited JSON over socket (not length-prefixed or msgpack)
- Socket cleanup: remove stale socket file on startup, write PID file for health checks

**File hints:**
- `src/server.rs` — Unix socket listener + dispatch
- `src/client.rs` — MCP stdio bridge
- `src/main.rs` — CLI entry point with clap
- `src/protocol.rs` — socket wire protocol types

**Depends on:** Phase 2 | **Unlocks:** Phase 4, Phase 5

**Done when:**
- [ ] DW-3.1: `grug serve` starts, creates socket at `~/.grug-brain/grug.sock`, accepts connections
- [ ] DW-3.2: `grug --stdio` connects to socket and correctly bridges all 9 MCP tools
- [ ] DW-3.3: Multiple concurrent `--stdio` sessions work without interference
- [ ] DW-3.4: Clean error message when `--stdio` can't connect (server not running)
- [ ] DW-3.5: Server removes stale socket file on startup
- [ ] DW-3.6: Integration tests: start server, connect client, exercise all tools through the socket

**Difficulty:** MEDIUM
**Uncertainty:** rmcp's `#[tool]` macro approach may not suit the forwarding pattern — may need manual JSON-RPC dispatch on the stdio side instead of using rmcp's server trait

---

### Phase 4: Git Sync + Background Services
**Model:** opus
**Skills:** code-foundations:code

**Goal:** Port all git sync logic and background service infrastructure from the current JS implementation.

**Scope:**
- IN: Git operations via CLI shell-out (async `tokio::process::Command`), sync locks (per-brain mutex), periodic sync timers (`tokio::time::interval`), refresh timers for read-only brains, rebase conflict resolution (detect REBASE_HEAD, save local version to conflicts/ category, abort + reset), .grugignore parsing and git exclude management, sync:false frontmatter respect, ensureGitRepo (init if needed), hostname detection, event loop heartbeat monitor, graceful shutdown (cancel all tasks on SIGTERM/SIGINT)
- OUT: No brew/plugin changes

**Approach notes:**
- Shell out to git CLI (not git2 crate) — user's existing SSH keys, credential helpers, and hooks work automatically
- Sync lock granularity is per-brain (not global) — matching current JS behavior
- Conflict resolution must match current behavior exactly: detect REBASE_HEAD, save local to conflicts/, abort rebase, reset to remote

**File hints:**
- `src/git.rs` — all git operations
- `src/services.rs` — timer management, background tasks
- Current `server.js` lines 238-458 (git sync), 746-834 (timers)

**Depends on:** Phase 3 | **Unlocks:** Phase 5

**Done when:**
- [ ] DW-4.1: Periodic git pull --rebase + push works for writable brains with remotes
- [ ] DW-4.2: Rebase conflict resolution: detects REBASE_HEAD, saves local to conflicts/, aborts rebase, resets to remote
- [ ] DW-4.3: .grugignore loaded and applied to git info/exclude
- [ ] DW-4.4: sync:false frontmatter files excluded from git operations
- [ ] DW-4.5: Per-brain sync timers with configurable intervals (minimum 10s)
- [ ] DW-4.6: Read-only brain refresh timers (minimum 3600s)
- [ ] DW-4.7: Graceful shutdown: SIGTERM/SIGINT cancels all timers, waits for in-flight git ops
- [ ] DW-4.8: Integration tests: git sync against a local bare repo, conflict resolution scenario

**Difficulty:** HIGH
**Uncertainty:** Git rebase conflict edge cases — the current JS handles several failure paths (no unmerged files, can't read local version, can't write conflict file). Must test each.

---

### Phase 5: Plugin + Brew Formula + Setup
**Model:** opus

**Goal:** Update the Claude Code plugin and create distribution tooling so `brew install` + `claude plugin add` + `/setup` gets a user from zero to working.

**Scope:**
- IN: Homebrew formula (Rust build from source or prebuilt binary), plugin.json update (mcpServers points to `grug --stdio`), setup.md rewrite (brew install, launchd/systemd service for `grug serve`), README rewrite for new installation flow
- OUT: No Rust code changes

**Constraints:**
- Plugin must not retain any reference to Bun or the JS server
- Brew formula must produce a single self-contained binary (no runtime deps)

**Approach notes:**
- Initial distribution via personal Homebrew tap — official core submission later
- `grug serve --install-service` writes the launchd plist / systemd unit itself (self-installing)

**File hints:**
- `.claude-plugin/plugin.json` — MCP server registration
- `commands/setup.md` — setup slash command
- `README.md` — user-facing docs
- `Formula/grug-brain.rb` or `HomebrewFormula/` — brew formula

**Depends on:** Phase 3, Phase 4 | **Unlocks:** None

**Done when:**
- [ ] DW-5.1: Homebrew formula builds the Rust binary and installs to PATH
- [ ] DW-5.2: plugin.json mcpServers entry points to `grug --stdio` (no bun dependency)
- [ ] DW-5.3: `grug serve --install-service` creates correct plist/unit file AND service is enabled (`launchctl list | grep grug` on macOS, `systemctl --user is-enabled grug` on Linux)
- [ ] DW-5.4: setup.md rewritten for: brew install, service installation, MCP registration, brain config
- [ ] DW-5.5: README updated with new install flow, architecture diagram, tool reference
- [ ] DW-5.6: In a clean CI environment (no pre-existing grug.db), `brew install <tap>/grug-brain && grug serve --install-service && grug --stdio` followed by a grug-recall tool call returns a valid MCP response

**Difficulty:** MEDIUM
**Uncertainty:** Homebrew tap setup — may need to create a separate repo for the tap

---

## Test Coverage

**Level:** All tool handlers and git sync paths covered by unit or integration tests. Target >= 80% line coverage (`cargo tarpaulin`), with remaining gap documented as untestable platform paths.

## Test Plan

- [ ] Unit: brain config parsing (valid, invalid, defaults, home expansion)
- [ ] Unit: frontmatter extraction, body extraction, description extraction
- [ ] Unit: file walker (.md/.mdx filtering, dot/underscore skip)
- [ ] Unit: FTS query building (single term, multi-term, empty)
- [ ] Unit: slugify, date helpers
- [ ] Unit: each of 9 tools against temp SQLite database
- [ ] Unit: config hot-reload mtime detection
- [ ] Integration: Unix socket server start/stop/connect
- [ ] Integration: MCP stdio bridge end-to-end (tool call -> socket -> response)
- [ ] Integration: concurrent client sessions
- [ ] Integration: git sync against local bare repo
- [ ] Integration: rebase conflict resolution scenario
- [ ] Integration: graceful shutdown (SIGTERM cancels timers)
- [ ] Integration: brew formula build + install

## Assumptions

| Assumption | Confidence | Verify Before Phase | Fallback If Wrong |
|-----------|-----------|--------------------|--------------------|
| rmcp transport-io works with custom tool forwarding | HIGH | Phase 3 | Manual JSON-RPC dispatch over stdio |
| rusqlite bundled FTS5 matches Bun's SQLite FTS5 behavior | HIGH | Phase 1 | Both use standard SQLite FTS5; should be identical |
| Homebrew allows personal tap without review | HIGH | Phase 5 | Distribute binary via GitHub releases + curl install |
| Unix socket handles 10+ concurrent connections | HIGH | Phase 3 | Trivially true for tokio — handles thousands |

## Decision Log

| Decision | Alternatives Considered | Rationale | Phase |
|----------|------------------------|-----------|-------|
| All Rust (not Go, not Zig) | Go server + Rust CLI, all Zig | User chose Rust for performance + single binary | All |
| Unix socket only (no HTTP) | HTTP REST API, both | Socket is faster, HTTP unnecessary for local-only | 3 |
| Server required (no standalone) | Standalone fallback, hybrid | Single code path, simplest design, brew service ensures uptime | 3 |
| Shell out to git (not git2) | git2 crate (libgit2) | Better auth, hooks, rebase support; no C build dep | 4 |
| rmcp for MCP stdio | Custom JSON-RPC impl | Official SDK, maintained by Anthropic, handles framing | 3 |
| Personal Homebrew tap (not core) | Homebrew core, cargo install only | Faster iteration, no review process | 5 |

---

## Notes

- Current `server.js` is 1715 lines. Rust version will likely be 2500-3500 lines due to explicit types and error handling, but split across modules.
- The existing `grug.db` database should be reusable — schema version 5 compatibility means users upgrading from JS to Rust keep their data.
- Dream tool is the most complex single tool (~150 lines of JS). Plan for thorough testing.
- The `grug-docs` tool is deprecated in current JS. Port it but mark deprecated in the tool description.

---

## Execution Log

_To be filled during /code-foundations:building_
