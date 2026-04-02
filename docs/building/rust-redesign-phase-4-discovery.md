# Discovery: Phase 4 - Git Sync + Background Services

## Files Found

### Existing Rust source (Phase 1-3 complete)
- `src/lib.rs` -- module declarations (no git or services modules yet)
- `src/main.rs` -- CLI with `serve` and `--stdio` commands
- `src/server.rs` -- Unix socket server with DB worker thread, accept loop, ctrl-c shutdown
- `src/config.rs` -- brain config loading (Brain.git, sync_interval, source, refresh_interval fields exist)
- `src/types.rs` -- Brain struct with git/sync_interval/source/refresh_interval fields
- `src/helpers.rs` -- slugify, today, paginate
- `src/parsing.rs` -- extract_frontmatter, extract_body, extract_description
- `src/walker.rs` -- walk_files, get_categories
- `src/tools/mod.rs` -- GrugDb struct, test_helpers
- `src/tools/indexing.rs` -- index_file, remove_file, sync_brain
- `src/tools/write.rs` -- grug_write (has `// Git commit skipped (Phase 4)` comment)
- `src/tools/delete.rs` -- grug_delete (has `// Git commit skipped (Phase 4)` comment)
- `src/tools/sync.rs` -- grug_sync (reindex only, no git)
- `src/protocol.rs` -- socket wire protocol
- `src/client.rs` -- MCP stdio bridge

### Current JS reference (server.js)
- Lines 238-272: git() helper, getHostname()
- Lines 246-256: syncLocks (per-brain Map)
- Lines 274-282: ensureGitRepo (init, .gitignore, commit)
- Lines 284-287: hasRemote
- Lines 289-308: loadGrugIgnore, isLocalFile (sync:false + patterns)
- Lines 310-337: syncGitExclude, gitCommitFile
- Lines 340-417: resolveRebaseConflict (complex -- multiple failure paths)
- Lines 419-458: gitSync (pull --rebase, push, dirty check, reindex)
- Lines 746-799: startBrainTimers (sync + refresh intervals, minimum clamping)
- Lines 801-813: stopBrainTimers
- Lines 827-834: heartbeat monitor

## Current State

- 145 tests passing (137 unit + 8 integration)
- Phase 3 left two explicit stubs: `// Git commit skipped (Phase 4)` in write.rs and delete.rs
- Brain type already carries all necessary fields (git, sync_interval, source, refresh_interval)
- Server has basic ctrl-c shutdown but no background task management
- No `src/git.rs` or `src/services.rs` exist yet
- DB worker thread pattern is established (mpsc channel + dedicated std::thread)
- Server's `run_server` already accepts `db_tx: mpsc::Sender<DbRequest>` -- background services will need a clone of this sender to submit reindex requests

## Gaps

1. **No git module** -- need to create `src/git.rs` with all git shell-out operations
2. **No services module** -- need to create `src/services.rs` for timer management and background tasks
3. **Server integration** -- `run_server` needs to spawn background services and shut them down gracefully; currently only handles ctrl-c to break accept loop
4. **Write/delete git integration** -- the `// Git commit skipped (Phase 4)` stubs need to become real calls (but these are fire-and-forget async operations that need the git module)
5. **Signal handling** -- current server only handles ctrl-c; need SIGTERM support and coordinated shutdown of background tasks
6. **DbRequest accessibility** -- `DbRequest` is `pub(crate)` which is fine; background services need to send reindex requests through the same channel

## Integration Points

The server currently has this shutdown flow:
```
ctrl_c signal -> break accept loop -> drop db_tx -> remove socket -> remove pid
```

It needs to become:
```
ctrl_c/SIGTERM -> cancel all background tasks -> wait for in-flight git ops -> break accept loop -> drop db_tx -> remove socket -> remove pid
```

Key insight: background git sync tasks need a `db_tx` clone to submit `grug-sync` tool calls for reindexing after git pull. The `DbRequest` struct and `dispatch_tool` function already handle this.

## Prerequisites
- [x] Required files exist (Brain type, config, server, tools)
- [x] Dependencies available (tokio with `full` features includes signal, process, time)
- [x] DB worker thread pattern established (Phase 3)
- [x] Brain fields for git sync present (git, sync_interval, source, refresh_interval)
- [x] Indexing functions exist (sync_brain in tools/indexing.rs)

## Recommendation
BUILD -- All prerequisites met. Two new files (git.rs, services.rs) plus modifications to server.rs and lib.rs. Write/delete git commit integration is a stretch goal but architecturally clean since those functions run on the DB thread and git commits should be async fire-and-forget.

Note on write/delete git commit: The current architecture has write/delete running synchronously on the DB thread. Git commits are async operations. Rather than making the DB thread async, the git commit after write/delete should be spawned as a background task (fire-and-forget) from the server layer, not from inside the tool functions. This matches the JS behavior where gitCommitFile is called after the tool returns its response.
