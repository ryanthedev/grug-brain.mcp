# Discovery: Phase 5 - Plugin + Brew Formula + Setup

## Files Found
- `.claude-plugin/plugin.json` — exists, currently references `bun` + `server.js --stdio`
- `commands/setup.md` — exists, 265 lines, references bun, npm, HTTP-based launchd/systemd services
- `commands/dream.md` — exists (out of scope)
- `commands/ingest.md` — exists (out of scope)
- `README.md` — exists, 150 lines, references bun-based workflow
- `src/main.rs` — exists, 55 lines, has `Cli` struct with `Commands::Serve` and `--stdio` flag
- `src/server.rs` — exists, `run_server` function with socket/db/config params
- `Cargo.toml` — exists, project name `grug-brain`, version `0.1.0`
- No `Formula/` or `HomebrewFormula/` directory exists yet

## Current State

### Rust Binary
All 4 prior phases complete. 166 tests pass (158 unit + 8 integration). Binary compiles. Two modes work:
- `grug serve [--socket PATH]` — Unix socket server with background services
- `grug --stdio` — MCP stdio bridge to running server

### CLI Argument Structure (main.rs)
```rust
enum Commands {
    Serve { socket: Option<PathBuf> },
}
```
No `--install-service` flag exists yet on the `Serve` variant.

### plugin.json
Points to `bun ${CLAUDE_PLUGIN_ROOT}/server.js --stdio`. Must change to `grug --stdio`.

### setup.md
Entirely bun-centric: bun install, bun runtime check, HTTP health checks, launchd/systemd with bun paths, MCP registration as HTTP transport. All must be rewritten for the Rust binary + Unix socket architecture.

### README.md
References bun-based install. Needs architecture diagram and updated install flow.

## Gaps
1. No `--install-service` flag on `grug serve` — must be added (Rust code change)
2. No Homebrew formula file exists
3. No CI test script for the install chain (DW-5.6)
4. plugin.json still references bun
5. setup.md still references bun and HTTP transport
6. README.md still references old architecture

## Prerequisites
- [x] Rust binary compiles and tests pass
- [x] `grug serve` and `grug --stdio` modes work
- [x] Git sync and background services implemented
- [x] All 9 MCP tools implemented

## Recommendation
BUILD — All prerequisites met. Implementation requires:
1. Add `--install-service` flag to `Serve` command in main.rs (Rust code change)
2. Create Homebrew formula (new file)
3. Update plugin.json
4. Rewrite setup.md
5. Rewrite README.md
6. Create CI test script for install chain
