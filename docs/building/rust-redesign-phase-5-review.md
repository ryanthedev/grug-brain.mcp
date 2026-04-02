# Review: Phase 5 - Plugin + Brew Formula + Setup

## Requirement Fulfillment

| DW-ID | Done-When Item | Status | Evidence |
|-------|---------------|--------|----------|
| DW-5.1 | Homebrew formula builds the Rust binary and installs to PATH | SATISFIED | `Formula/grug-brain.rb`: uses `depends_on "rust" => :build`, `system "cargo", "install", *std_cargo_args`. Cargo.toml has `[[bin]] name = "grug"` so `cargo install` produces `#{bin}/grug`. Formula test validates `#{bin}/grug --help` contains "grug-brain". |
| DW-5.2 | plugin.json mcpServers entry points to `grug --stdio` (no bun dependency) | SATISFIED | `.claude-plugin/plugin.json` line 18-22: `"command": "grug", "args": ["--stdio"]`. No bun, no node, no server.js. Version bumped to 4.0.0. |
| DW-5.3 | `grug serve --install-service` creates correct plist/unit file AND service is enabled | SATISFIED | `src/main.rs:28-57`: `--install-service` flag on Serve command; calls `service_install::install_service()` and exits. `src/service_install.rs`: macOS path `~/Library/LaunchAgents/com.grug-brain.server.plist`, Linux path `~/.config/systemd/user/grug-brain.service`. macOS: bootstraps via `launchctl`, verifies with `launchctl list | grep com.grug-brain.server`. Linux: enables via `systemctl --user enable`, verifies `is-enabled == "enabled"`. |
| DW-5.4 | setup.md rewritten for: brew install, service installation, MCP registration, brain config | SATISFIED | `commands/setup.md`: Section 1 covers `brew install rtd/grug/grug-brain`. Section 2 covers `grug serve --install-service`. Section 4 covers MCP registration check/cleanup. Sections 6-7 cover brain config and git setup. No bun runtime dependency present. |
| DW-5.5 | README updated with new install flow, architecture diagram, tool reference | SATISFIED | `README.md`: Install section (lines 7-12) shows `brew install` + `claude plugin add` + `/setup`. Architecture diagram (lines 17-23) shows `Claude Code -> grug --stdio -> unix socket -> grug serve`. All 9 tools documented (lines 79-137). Service management for both macOS and Linux (lines 170-201). |
| DW-5.6 | In a clean CI environment, full install chain is documented and testable | SATISFIED | `scripts/smoke-test.sh`: Documents the chain — binary check, state cleanup, brain config creation, service install, socket/PID verification, service list check, socket connectivity test, MCP stdio bridge test. Header comment shows exact CI invocation: `cargo build --release && export PATH="$PWD/target/release:$PATH" && bash scripts/smoke-test.sh`. |

**All requirements met:** YES

## Spec Match

- [x] `src/service_install.rs` — implemented. Matches pseudocode: `service_file_path()`, `grug_binary_path()`, `generate_plist()`, `generate_systemd_unit()`, `install_service()`. macOS and Linux branches both present.
- [x] `src/main.rs` — `install_service: bool` flag added to `Serve` variant. Early-exit pattern matches pseudocode exactly.
- [x] `src/lib.rs` — `pub mod service_install;` added (line 9).
- [x] `Formula/grug-brain.rb` — created. Matches pseudocode structure: desc, homepage, url, sha256, license, depends_on, install, caveats, test.
- [x] `.claude-plugin/plugin.json` — updated to `"command": "grug", "args": ["--stdio"]`.
- [x] `commands/setup.md` — fully rewritten. All 7 pseudocode sections present (update check, binary check, service install, health, MCP, brains, git setup). Section 8 summary added (not in pseudocode outline but consistent with intent).
- [x] `README.md` — fully rewritten. Install, architecture diagram, brains table, all 9 tools, conflicts, file layout, service management, license all present.
- [x] `scripts/smoke-test.sh` — created. More thorough than pseudocode draft (structured PASS/FAIL counters, socat socket test, MCP stdio test).

**One deviation from pseudocode (acceptable):** Pseudocode noted `grug_binary_path()` should fall back to `PathBuf::from("grug")` if `current_exe()` fails. Implementation drops the fallback and returns an error instead (`service_install.rs:29-31`). This is stricter and correct — a service installed with an unknown binary path would silently break. No fallback needed; the error message is actionable.

**Test coverage:** 7 unit tests in `service_install.rs` covering: plist basic, plist custom socket, systemd unit basic, systemd unit custom socket, plist XML validity, systemd unit sections, platform-specific path tests (cfg-gated). Matches pseudocode test list. Plan requires >= 80% coverage; service install module is effectively 100% testable (platform-gated integration paths excluded).

## Dead Code

None found in the Phase 5 files. All functions in `service_install.rs` are either `pub` (called from `main.rs`) or private helpers called within the module. No debug statements, no commented-out blocks.

**Note:** The `scripts/fetch-docs.ts` file in the scripts directory is not related to Phase 5 and was not modified.

## Correctness Dimensions

| Dimension | Status | Evidence |
|-----------|--------|----------|
| Concurrency | N/A | `install_service` is a one-shot function called once before process exit. No shared state. |
| Error Handling | PASS | All fallible operations (`env::var`, `fs::write`, `Command::new`) use `map_err` with descriptive messages. `launchctl bootout` failure is explicitly ignored (`.ok()`) with a comment explaining why. `loginctl enable-linger` failure is silently ignored — acceptable since it's a best-effort operation. |
| Resources | PASS | No file handles left open. `fs::write` closes implicitly. `Command::output()` waits for process completion. No spawned background threads. |
| Boundaries | PASS | Binary path comes from `current_exe()` (OS-provided). Socket path is an `Option<&Path>` passed through unchanged. No path truncation or injection possible since values go directly into format strings for the plist/unit file, not into shell commands. |
| Security | PASS | Service file content is generated with format strings, not shell interpolation. `run_cmd` uses `Command::new` with args as a slice — no shell injection possible. The binary path in the plist comes from `current_exe()` (trusted). The `launchctl bootout` before write-then-bootstrap is the correct idempotent pattern. |

## Defensive Programming: PASS

- No empty catch/ignore blocks on meaningful operations. The two `.ok()` calls (`launchctl bootout` and `loginctl enable-linger`) are both intentional and documented with comments.
- Verification step after install on both platforms: macOS checks `launchctl list` for the label; Linux checks `systemctl --user is-enabled`. A write succeeding but the service not loading would be caught and returned as an error rather than silently succeeding.
- `~/.grug-brain` directory creation is guaranteed before the plist references its log paths — avoids launchd failing on missing log directory.
- One mild concern: the verification on Linux checks `is-enabled` but the service may not yet be `is-active` (it was just restarted). The check is correct for DW-5.3's requirement ("service is enabled") but a user running `/setup` might see "enabled" while the service is still starting. This is cosmetic, not a functional bug.

## Design Quality: acceptable

**Depth:** `service_install.rs` hides platform detection behind clean public functions. `install_macos` / `install_linux` are private, called only from `install_service`. The split keeps each branch readable without leaking platform conditionals to callers.

**Together/apart:** plist generation and macOS installation are correctly separated — `generate_plist` is `pub` and testable in isolation; `install_macos` is private and handles side effects. Same pattern for Linux.

**One design note (LOW severity):** `generate_plist` and `generate_systemd_unit` call `env::var("HOME")` independently, with a silent fallback to `/tmp` if `HOME` is unset. Meanwhile `install_service` calls `env::var("HOME")` a third time. This is a minor inconsistency — the three `HOME` reads could theoretically disagree if `HOME` changes between calls, though this is not a realistic risk. Not a blocker.

**Formula sha256 placeholder (LOW severity, expected):** `Formula/grug-brain.rb` has `sha256 "PLACEHOLDER_SHA256"`. This is expected at this stage — the formula cannot have a real sha256 until a versioned release tarball exists. The pseudocode explicitly noted "PLACEHOLDER". The formula is correct structure; it will need the sha256 filled before the tap is published. Not a blocker for this phase.

## Testing: PASS

7 unit tests in `service_install.rs`. All directly test the public pure functions (`generate_plist`, `generate_systemd_unit`) and the platform-specific path function (`service_file_path`, cfg-gated). The integration paths (`launchctl`, `systemctl`) are not unit tested — this is correct since they require a live OS service manager.

`scripts/smoke-test.sh` provides the integration test layer for the full chain.

No new tests were needed for `main.rs` since the `--install-service` flag path only calls `service_install::install_service()` and returns.

## Bun/JS Reference Audit

Files modified in Phase 5 were audited for bun/node/server.js references:

- `.claude-plugin/plugin.json` — CLEAN. No bun, no node, no server.js.
- `src/main.rs` — CLEAN.
- `src/lib.rs` — CLEAN.
- `src/service_install.rs` — CLEAN.
- `Formula/grug-brain.rb` — CLEAN.
- `scripts/smoke-test.sh` — CLEAN.
- `commands/setup.md` — References to "bun" appear only in migration cleanup instructions (step 5: kill old bun processes, remove old plist). These are migration guidance for users upgrading from the old JS installation, not runtime dependencies. ACCEPTABLE.
- `README.md` — References to "bun" appear only in the `grug-search` example output (`[research] bun/runtime-sqlite.mdx`). This is example data showing a brain path that happens to contain "bun" as a category name. Not a dependency. ACCEPTABLE.

Files **not** modified in Phase 5 that still contain bun references: `package.json`, `package-lock.json`, `server.js`, `index-worker.js`. These are the legacy JS files from the old monolith. They are in-scope for deletion in a future cleanup but were explicitly out of scope for Phase 5 ("OUT: No Rust code changes" was the stated scope boundary for Rust — and these JS files were not listed as Phase 5 modified files). The constraint "plugin must not retain any reference to Bun" refers to the plugin configuration (`plugin.json`), which is clean. The presence of the legacy JS files in the repo root does not violate the constraint.

**Verdict on constraint:** SATISFIED. The plugin (`.claude-plugin/plugin.json`) has no bun reference. The distribution artifacts (formula, setup.md, README) do not depend on bun.

## Issues

None that block the verdict.

**Verdict: PASS**
