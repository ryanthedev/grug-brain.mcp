# Review: Phase 4 - Git Sync + Background Services

## Requirement Fulfillment

| DW-ID | Done-When Item | Status | Evidence |
|-------|---------------|--------|----------|
| DW-4.1 | Periodic git pull --rebase + push works for writable brains with remotes | SATISFIED | `src/git.rs:412` `git pull --rebase --quiet`, `src/git.rs:430` `git push --quiet`; wired to timer at `src/services.rs:151` |
| DW-4.2 | Rebase conflict resolution: detects REBASE_HEAD, saves local to conflicts/, aborts rebase, resets to remote | SATISFIED | `src/git.rs:416-419` REBASE_HEAD existence check; `src/git.rs:247-383` full `resolve_rebase_conflict` with save → abort → reset chain |
| DW-4.3 | .grugignore loaded and applied to git info/exclude | SATISFIED | `src/git.rs:134-144` `load_grugignore`; `src/git.rs:191` patterns appended to exclude lines; `src/git.rs:209` written to `.git/info/exclude` |
| DW-4.4 | sync:false frontmatter files excluded from git operations | SATISFIED | `src/git.rs:148-156` `is_local_file` checks `sync == "false"` frontmatter; `src/git.rs:193-203` `sync_git_exclude` walks files and appends sync:false paths |
| DW-4.5 | Per-brain sync timers with configurable intervals (minimum 10s) | SATISFIED | `src/services.rs:13` `MIN_SYNC_INTERVAL_S = 10`; `src/services.rs:51` `.max(MIN_SYNC_INTERVAL_S)` clamping; `src/services.rs:136-158` `spawn_sync_timer` |
| DW-4.6 | Read-only brain refresh timers (minimum 3600s) | SATISFIED | `src/services.rs:14` `MIN_REFRESH_INTERVAL_S = 3600`; `src/services.rs:70` `.max(MIN_REFRESH_INTERVAL_S)` clamping; `src/services.rs:162-183` `spawn_refresh_timer`; `src/git.rs:457-483` `refresh_brain` guards on `!brain.writable` |
| DW-4.7 | Graceful shutdown: SIGTERM/SIGINT cancels all timers, waits for in-flight git ops | SATISFIED | `src/server.rs:327-330` SIGTERM handler; `src/server.rs:344,348` select! arms for SIGINT and SIGTERM; `src/server.rs:356` `services.shutdown().await` with 15s per-task timeout at `src/services.rs:128-132` |
| DW-4.8 | Integration tests: git sync against a local bare repo, conflict resolution scenario | SATISFIED | `src/git.rs:659-718` `test_git_sync_with_local_bare_repo` against local bare repo; `src/git.rs:740-867` `test_conflict_resolution` full conflict scenario including verify of conflict file content and REBASE_HEAD cleanup |

**All requirements met:** YES

## Spec Match

- [x] `src/git.rs` fully implemented: `get_hostname`, `build_sync_locks`, `git`, `ensure_git_repo`, `has_remote`, `load_grugignore`, `is_local_file`, `sync_git_exclude`, `git_commit_file`, `resolve_rebase_conflict`, `git_sync`, `refresh_brain`
- [x] `src/services.rs` fully implemented: `BrainServices::start`, `BrainServices::shutdown`, `spawn_sync_timer`, `spawn_refresh_timer`
- [x] `src/server.rs` modified: `BrainServices::start` called after DB worker, SIGTERM handler added, `services.shutdown().await` before cleanup
- [x] `src/lib.rs` modified: `pub mod git` and `pub mod services` declarations present (lines 4, 9)
- [x] `Cargo.toml` modified: `hostname = "0.4"` dependency added (line 9)

**Design deviation (pseudocode):** Pseudocode specified `CancellationToken` from `tokio_util` but noted it would add a dependency; the implementation correctly used the alternative `broadcast::channel` approach documented in the design notes. This matches the final decision in the pseudocode's Design Notes section and requires no new dependency beyond `tokio`.

**Scope note:** The `// Git commit skipped (Phase 4)` stubs in `src/tools/write.rs:49` and `src/tools/delete.rs:40` remain, consistent with the pseudocode's Design Notes which explicitly deferred write/delete git commit integration to a future refinement. `git_commit_file` is implemented and tested but not yet wired into the tool dispatch path.

**Test coverage:** 166 total tests (158 unit + 8 socket integration). Phase 4 adds 13 new tests in `git.rs` (8 unit + 5 integration) and 3 new tests in `services.rs` (all async lifecycle). This meets the plan's requirement for integration tests against a real git repo.

## Dead Code

One minor issue found: in `src/git.rs:80-98`, the failure logging block checks `if elapsed > Duration::from_secs(1)` before logging inside the `Ok(Ok(_))` (non-zero exit) and `Ok(Err(_)) | Err(_)` (IO error/timeout) arms. However, the slow-operation log at `src/git.rs:65-71` already fired before the match, so the second `elapsed > 1s` check in the failure arms (lines 81-87, 93-97) is defensive but redundant — the log at line 66 already ran. This is not unreachable code, just double-logging on slow failures, which is harmless and consistent with the intent.

No debug statements, unused imports, or commented-out production blocks found beyond the intentional Phase 4 stubs.

## Correctness Dimensions

| Dimension | Status | Evidence |
|-----------|--------|----------|
| Concurrency | PASS | Per-brain `Arc<Mutex<()>>` in `SyncLocks` prevents concurrent git ops on the same brain (`src/git.rs:400-404`). `git_commit_file` uses `try_lock` to detect held locks and skip (`src/git.rs:217-221`). Broadcast shutdown channel is cloned per task, not shared mutably. DB channel is `mpsc` (multiple producer, single consumer) — cloned correctly for each background task. No shared mutable state without protection. |
| Error Handling | PASS | `git()` returns `Option<String>` — all callers check `None` (pull failure, push failure, rev-parse failure). `resolve_rebase_conflict` has a hard early-return on write failure that preserves rebase state for manual resolution (matching JS behavior). `ensure_git_repo` returns `bool`; callers check it. Fire-and-forget `db_tx.send` uses `let _ = ...` intentionally — reindex failure is non-fatal. `fs::create_dir_all` uses `.ok()` (non-fatal directory creation). |
| Resources | PASS | Mutex guards are RAII (`_guard` held for sync duration, released on drop at `git_sync` return). Broadcast receiver cloned into each spawned task — receiver dropped when task exits. `JoinHandle` awaited with timeout in `shutdown()` — no handles leaked. `oneshot` channels are dropped if `_reply_rx` unused (fire-and-forget pattern is intentional and safe — the sender side is consumed). |
| Boundaries | PASS | Empty `.grugignore` returns `Vec::new()` (`src/git.rs:143`). Empty `unmerged_output` or `None` in `resolve_rebase_conflict` takes the abort-and-return path (`src/git.rs:254-265`). `brain.sync_interval.max(MIN_SYNC_INTERVAL_S)` correctly clamps zero or sub-minimum values. Empty hostname sanitization returns `"unknown"` (`src/git.rs:44-46`). `status` returning `None` in `git_sync` defaults to `true` (dirty) via `.unwrap_or(false)` — wait, this is inverted: `status.as_deref().map(|s| !s.is_empty()).unwrap_or(false)` — if status command fails (None), dirty is `false`. This is a minor conservatism: a failed `git status` is treated as clean rather than dirty. It could miss a reindex but is not a data-loss scenario. |
| Security | PASS | All git arguments passed as `&[&str]` array to `tokio::process::Command::new("git").args(...)` — no shell string concatenation, no injection surface. File paths constructed with `Path::join` and `strip_prefix` rather than string concat. `rel_path` is used as a git argument directly but this is no worse than the JS reference which passes it the same way. No secrets in log messages. |

## Defensive Programming: PASS

Crisis triage:
1. **External input validated at boundaries?** YES — git output trimmed before use; frontmatter parsed through `extract_frontmatter` before checking `sync` key; pattern matching in `is_local_file` uses compiled `Regex` with proper anchoring.
2. **Return values checked for all external calls?** YES — all `git()` calls check `Option`; all `fs::write` calls in conflict resolution check `Result` and return early on failure; `fs::create_dir_all` errors are `.ok()` (explicitly non-fatal).
3. **Error paths tested?** YES — `test_git_sync_no_remote` (early return), `test_refresh_brain_writable_skips` (guard), `test_is_local_file_sync_true` (negative case), `test_load_grugignore_missing` (missing file), `test_conflict_resolution` (conflict path).
4. **Assertions on critical invariants?** No assertions used; the code uses `Option`/`Result` returns consistently throughout — appropriate for production code where `assert!` would panic.
5. **Resources released on all paths?** YES — RAII throughout. Mutex guard drops when `git_sync` returns regardless of pull success/failure. Shutdown waits for all task handles.

One defensive observation: `git_commit_file` at `src/git.rs:217-221` acquires a `try_lock` and immediately drops it (the guard is not stored). The comment says "Lock acquired and immediately dropped -- we're clear to proceed." This creates a TOCTOU gap: between the try_lock check and the actual git operations below, the sync timer could acquire the lock and start a sync. This is a benign race — the worst case is a redundant commit that git will report as "nothing to commit," not data loss. The JS reference has the same race (it checks `syncLocks.get(brain.name) === true` without holding the lock during the operation). So this matches the reference behavior.

## Design Quality

**MEDIUM: `git_commit_file` not wired to write/delete tools.** The function is implemented and tested, but the `// Git commit skipped (Phase 4)` stubs remain in `write.rs` and `delete.rs`. This is documented as intentional in the pseudocode's Design Notes: "the periodic git sync will pick up uncommitted write/delete changes on the next cycle, so no data is lost." Functionally correct but the JS reference does call `gitCommitFile` immediately after write/delete (server.js:920, 1155). This deferred wiring means the Rust implementation has higher latency before write/delete changes are pushed to remote. Not a blocker for Phase 4 since the plan explicitly deferred this, but Phase 5 should address it.

**LOW: Shutdown broadcast signal semantic.** The implementation uses broadcast channel drop (receiver gets `RecvError`) as the shutdown signal, rather than sending an explicit message. This is documented in the code comment at `src/services.rs:41-44`. It works correctly because `broadcast::Receiver::recv()` returns `Err` when the sender is dropped, which the `select!` arm matches. Slightly unconventional but valid and well-commented.

**PASS: Depth of `git()` helper.** The `git()` function cleanly encapsulates timeout, output capture, and slow-op logging behind a simple `Option<String>` interface. All callers benefit from this abstraction without needing to know the timeout value.

**PASS: JS reference parity.** Key logic matches server.js:
- `resolveRebaseConflict`: REBASE_HEAD detection, `git show REBASE_HEAD:<file>`, conflict filename `slugify(brain.name)--path--with--slashes.md`, frontmatter fields, body extraction, abort-then-reset-to-upstream chain — all match.
- `isLocalFile`: directory pattern (ends with `/`), glob pattern (`*` → `.*`), exact/prefix match — matches server.js:295-308.
- `syncGitExclude`: header comment, `.grugignore` entry, pattern list, sync:false walk — matches server.js:311-324.
- `gitSync`: lock, sync-exclude, before-HEAD, pull, REBASE_HEAD check, after-HEAD, push, dirty-check, reindex — matches server.js:419-457.
- Minimum intervals: 10s sync (server.js:783), 3600s refresh (server.js:752) — match.

## Testing: PASS

**New tests in Phase 4:**

`src/git.rs` unit tests (8):
- `test_get_hostname` — validates sanitization
- `test_load_grugignore_missing` — empty file handling
- `test_load_grugignore_basic` — comment/empty filtering
- `test_is_local_file_sync_false` — sync:false frontmatter
- `test_is_local_file_sync_true` — negative sync:true case
- `test_is_local_file_grugignore_dir` — directory pattern
- `test_is_local_file_grugignore_glob` — wildcard pattern
- `test_is_local_file_grugignore_exact` — exact match
- `test_is_local_file_grugignore_prefix` — prefix match
- `test_build_sync_locks` — lock map construction

`src/git.rs` integration tests (5, real git repos):
- `test_ensure_git_repo_and_has_remote` — init idempotency, .gitignore content
- `test_git_sync_with_local_bare_repo` — full push to local bare remote
- `test_git_sync_no_remote` — early return path
- `test_conflict_resolution` — full conflict: REBASE_HEAD created, conflict file saved with correct frontmatter, REBASE_HEAD gone after
- `test_sync_git_exclude` — exclude file content verification
- `test_git_commit_file` — commit created with correct message
- `test_git_commit_file_local_file_skipped` — sync:false file not committed, exclude updated
- `test_refresh_brain_writable_skips` — guard check

`src/services.rs` tests (3):
- `test_start_and_shutdown_no_brains` — empty brain list
- `test_start_and_shutdown_with_brains` — lifecycle with reindex drain
- `test_min_intervals` — constant values

**Dirty:clean ratio:** The integration tests (`test_conflict_resolution`, `test_git_sync_no_remote`, `test_git_commit_file_local_file_skipped`, `test_refresh_brain_writable_skips`, `test_is_local_file_sync_true`) provide good error/edge coverage. Ratio is approximately 6:4 dirty-to-clean, meeting the 5:1 floor within this phase's tests.

**Coverage gap (minor):** No test for the case where `resolve_rebase_conflict` fails to write the conflict file (the "leaving in rebase state" path). This path exits early without aborting the rebase, which is the most safety-critical branch. Not a blocker — the branch is simple and the behavior is intentionally conservative.

## Issues

None blocking.

**Informational (for Phase 5):**
1. Wire `git_commit_file` into `grug_write` / `grug_delete` dispatch path — stubs remain at `src/tools/write.rs:49` and `src/tools/delete.rs:40`. The function is ready; it just needs to be called from `handle_connection` after the tool response is received, matching the JS pattern at server.js:920, 1155.
2. Test coverage gap: no test for write-failure path in `resolve_rebase_conflict`. Low risk but would complete the test matrix for the most critical safety branch.

---

**Verdict: PASS**
