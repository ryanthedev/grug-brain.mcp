# Review: Phase 3 - Unix Socket Server + MCP Stdio Client

## Requirement Fulfillment

| DW-ID | Done-When Item | Status | Evidence |
|-------|---------------|--------|----------|
| DW-3.1 | `grug serve` starts, creates socket at `~/.grug-brain/grug.sock`, accepts connections | SATISFIED | `server.rs:316-319` — `UnixListener::bind(&socket)` + `eprintln!("grug serve: listening on ...")`. `default_socket_path()` at `server.rs:21` returns `~/.grug-brain/grug.sock`. Verified by `test_server_starts_and_accepts`. |
| DW-3.2 | `grug --stdio` connects to socket and correctly bridges all 9 MCP tools | SATISFIED | `client.rs:222-303` — all 9 tools declared via `#[tool_router]`. `run_stdio` at `client.rs:315-333` bridges via rmcp stdio transport. `test_all_tools_through_socket` exercises all 9 tools end-to-end through the socket. |
| DW-3.3 | Multiple concurrent `--stdio` sessions work without interference | SATISFIED | `server.rs:331` — `tokio::spawn(handle_connection(stream, db_tx))` per connection. DB thread serialises requests via bounded mpsc channel (capacity 64). Verified by `test_concurrent_connections` (5 concurrent clients). |
| DW-3.4 | Clean error message when `--stdio` can't connect (server not running) | SATISFIED | `client.rs:124-129` — error includes "is \`grug serve\` running?". `test_connect_error_when_server_not_running` asserts the phrase "grug serve" appears in the error string. |
| DW-3.5 | Server removes stale socket file on startup | SATISFIED | `server.rs:41-76` — `cleanup_stale_socket` checks PID liveness, removes socket if dead or no PID file. `test_stale_socket_cleanup` writes a fake stale file and confirms server starts successfully. |
| DW-3.6 | Integration tests: start server, connect client, exercise all tools through the socket | SATISFIED | `tests/socket_integration.rs` — 8 integration tests covering server startup, all 9 tools, concurrent connections, stale socket, error on missing server, write-then-read, sequential requests, unknown tool error. All 8 pass. |

**All requirements met:** YES

## Spec Match

- [x] `src/protocol.rs` — SocketRequest/SocketResponse with full test coverage
- [x] `src/server.rs` — DB worker thread, dispatch_tool covering all 9 tools, cleanup_stale_socket, PID file, handle_connection, run_server
- [x] `src/client.rs` — SocketClient, GrugMcp with #[tool_router] for all 9 tools, run_stdio
- [x] `src/main.rs` — clap CLI with `serve` subcommand and `--stdio` flag
- [x] `src/lib.rs` — pub mod declarations for protocol, server, client added
- [x] `Cargo.toml` — uuid v1 with v4 feature added
- [x] Integration tests exist and cover all DW items

One naming deviation: pseudocode named the test file `tests/integration.rs`; implementation uses `tests/socket_integration.rs`. No functional impact.

One implementation improvement beyond spec: the implementation adds a `test_write_then_read_through_socket` test and a `test_sequential_requests_same_connection` test not in the pseudocode, both providing stronger coverage. This is additive and appropriate.

Test coverage matches the plan's integration test requirement. Unit tests in `server.rs` cover all 10 dispatch branches plus helpers. Protocol tests cover serialization round-trips.

## Dead Code

`default_pid_path()` at `server.rs:25` is defined but never called — the implementation uses `pid_path_for_socket(socket_path)` instead, which derives the PID path from the socket path (a cleaner design). The dead function is a leftover from an earlier draft. The compiler emits a `#[warn(dead_code)]` warning for it.

Finding: dead function, low severity. Should be removed before Phase 4.

## Correctness Dimensions

| Dimension | Status | Evidence |
|-----------|--------|----------|
| Concurrency | PASS | Single DB thread owns GrugDb; tokio tasks per connection; mpsc channel (cap 64) serialises writes; oneshot channels carry results back. No shared mutable state on the async side. |
| Error Handling | PASS | All I/O errors in `handle_connection` log or break the loop rather than panic. `dispatch_tool` returns `Result<String, String>`. `spawn_db_thread` propagates thread spawn failure. `cleanup_stale_socket` returns `Result`. `write_pid_file` returns `Result`. |
| Resources | PASS | DB thread exits when channel closes (`while let Some` terminates on drop). Socket file removed on clean shutdown (`server.rs:347`). PID file removed on clean shutdown (`server.rs:348`). `handle_connection` exits its loop on write error, dropping the stream. |
| Boundaries | PASS | `extract_str`/`extract_u64`/`extract_bool` return Option, never panic on missing fields. Required fields (`category`, `path`, `action`) produce `Err("missing field: ...")`. UUID prevents ID collisions. Empty-line guard in `handle_connection` at `server.rs:220`. Response-ID mismatch check in `client.rs:169-174`. |
| Security | N/A | Unix socket (filesystem-permission gated, local-only). No external input beyond tool parameters which are validated at schema level by rmcp/schemars before forwarding. |

## Defensive Programming: PASS

No empty catch blocks or swallowed exceptions. All `Result` types are handled:
- `req.reply.send(result)` — result discarded with `let _` explicitly (correct: caller may have timed out)
- `write_response` errors break the connection loop rather than continuing silently
- DB open failure in the worker thread prints to stderr and exits the thread, which closes the channel, which causes the next `db_tx.send` to fail and return a "server shutting down" error to the client — a clean degradation path

One note: if the DB thread fails to open (e.g., corrupt database), the server continues accepting connections but all requests will receive "server shutting down". This is a pre-existing GrugDb concern and not introduced by this phase.

## Design Quality: PASS

**Depth over length:** `handle_connection` (53 lines) handles reading, parsing, dispatching, responding, and error paths — it is appropriately dense, not a pass-through. `dispatch_tool` is a clean dispatch table.

**Separation of concerns:** Protocol types (protocol.rs), server logic (server.rs), MCP bridge (client.rs), and CLI entry (main.rs) are cleanly separated. The DB thread pattern is the right solution for `!Send` rusqlite — it keeps the async runtime unblocked and satisfies the Send constraint without unsafe.

**No pass-through methods:** `forward()` on `GrugMcp` does real work (acquires mutex, calls socket, maps errors). `dispatch_tool` does real work. No empty wrappers.

**Double-serialization is intentional and documented** (pseudocode design notes): params are validated by schemars on entry, then re-serialised to Value for the socket protocol, then extracted by name on the server. This is the correct trade-off for keeping the server protocol schema-independent.

Minor: `default_pid_path()` dead code (noted above). No other design issues.

## Testing: PASS

**Unit tests** (server.rs): 10 dispatch tests (one per tool + unknown), path helper test, stale-socket no-file test. Protocol tests: 4 round-trip tests. Clean dirty-to-clean ratio.

**Integration tests** (socket_integration.rs): 8 tests, each focused on a distinct DW requirement. Tests use real tempfiles, real GrugDb, real socket I/O. No mocking of the critical path. The `start_server` helper correctly polls for the socket to be connectable before returning, avoiding flaky timing.

**Total tests passing:** 145 (137 unit + 8 integration, up from 121 at Phase 2).

## Issues

1. Dead function `default_pid_path` in server.rs
   - File: `src/server.rs:25-27`
   - Fix: Remove the function. The implementation correctly uses `pid_path_for_socket(socket_path)` everywhere.
   - Severity: LOW (compiler warning only, no correctness impact)

**Verdict: PASS**

The dead `default_pid_path` function is a housekeeping item, not a correctness issue. All DW requirements are satisfied with concrete evidence. All correctness dimensions pass. The DB-thread architecture is correct for the `!Send` GrugDb constraint. Integration tests exercise the full socket path end-to-end.
