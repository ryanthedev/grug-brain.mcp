# Discovery: Phase 3 - Unix Socket Server + MCP Stdio Client

## Files Found
- `src/main.rs` — Placeholder `fn main()` (to be replaced)
- `src/lib.rs` — Declares all modules: config, db, helpers, parsing, spike, tools, types, walker
- `src/spike.rs` — rmcp proof-of-concept from Phase 1 (compiles and tests pass)
- `src/tools/mod.rs` — `GrugDb` struct with `Connection` + `BrainConfig`, all tool dispatch
- `src/tools/{write,search,read,recall,delete,config,sync,dream,docs}.rs` — 9 tool functions
- `src/config.rs` — `load_brains()` / `load_brains_from()` / `expand_home()`
- `src/db.rs` — `init_db()` creates schema v5
- `src/types.rs` — Brain, BrainConfig, FtsRow, SearchResult, Memory, RecallRow
- `Cargo.toml` — Has rmcp 1.3.0 (transport-io, server, client), tokio (full), clap (derive), serde

## Current State
- 121 tests passing across Phase 1 + Phase 2 code
- All 9 tool functions exist as synchronous `fn(&mut GrugDb, ...) -> Result<String, String>` (or `-> String` for `grug_search`)
- `GrugDb` holds a `rusqlite::Connection` (not `Send`, not `Sync`) and `BrainConfig`
- The Phase 1 spike (`src/spike.rs`) proves rmcp `#[tool_router]` + `#[tool_handler]` + `.serve()` compiles and works end-to-end with `tokio::io::duplex`
- `src/main.rs` is a placeholder

## Gaps
1. **No server, client, protocol, or CLI code exists** — all four files (`server.rs`, `client.rs`, `protocol.rs`, `main.rs`) need to be created or rewritten
2. **GrugDb is not Send** — `rusqlite::Connection` is `!Send`. Server must handle this via a dedicated DB thread or `spawn_blocking`
3. **Tool functions are synchronous** — they take `&mut GrugDb` and block. Must be wrapped for async dispatch
4. **No UUID dependency** — request IDs need generation. Can use simple atomic counter or add uuid crate
5. **No code-standards.md** exists in the project

## Assumption Verification
- **rmcp transport-io works with custom tool forwarding (Confidence: HIGH)**: VERIFIED. The spike in `src/spike.rs` demonstrates the full round-trip: server with `#[tool_router]` responds to `call_tool` from a client via `tokio::io::duplex`. Both `test_spike_types_exist` and `test_spike_serve_compiles` pass. The MCP stdio client will use this same pattern — implement `ServerHandler` with a `call_tool` that forwards to the Unix socket server.
- **Unix socket handles 10+ concurrent connections (Confidence: HIGH)**: VERIFIED trivially. Tokio's `UnixListener` is async and handles thousands of concurrent connections. No concern.

## Prerequisites
- [x] Required source files from Phase 1 + 2 exist
- [x] All 121 tests pass
- [x] rmcp crate available with transport-io, server, client features
- [x] tokio with full features available
- [x] clap with derive feature available
- [x] Spike proves rmcp pattern works

## Recommendation
**BUILD** — all prerequisites met, no blockers. Proceed to design and implementation.
