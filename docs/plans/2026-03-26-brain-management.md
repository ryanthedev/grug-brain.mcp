# Plan: Brain Management and Resilience

**Created:** 2026-03-26
**Status:** ready
**Complexity:** medium

---

## Context

The unified brain system works but has operational gaps. Users can't add brains without editing JSON and restarting. Ingested docs go to the plugin cache (lost on update). Read-only docs can't auto-refresh from upstream. These gaps make the first-time and ongoing user experience fragile.

## Constraints

- Single-file server (server.js)
- Config at `~/.grug-brain/brains.json` — no env var fallback
- Ingested docs default to `~/.grug-brain/<name>/` (survives plugin updates)
- grug-config is an MCP tool, not just a slash command
- Lazy config reload (re-read brains.json on tool calls) — no file watchers
- Optional `refreshInterval` in brains.json for auto-updating read-only brains

## Chosen Approach

**Lazy Reload + grug-config tool**

Re-read `brains.json` on tool calls to pick up external changes. `grug-config` tool provides add/remove/list actions and can start sync timers for newly added brains. `/ingest` updated to default to `~/.grug-brain/<name>/`. Optional `refreshInterval` field enables periodic `git pull` for read-only doc brains.

**Fallback:** If lazy reload adds noticeable latency, add mtime-based caching (read only when file changed).

## Rejected Approaches

- **File watcher (fs.watch):** Unreliable cross-platform. Adds complexity for marginal benefit over lazy reload.
- **Slash command only:** LLMs can't manage brains mid-conversation without user invoking a command.

---

## Implementation Phases

### Phase 1: Lazy Config Reload + grug-config Tool
**Model:** sonnet

**Goal:** Make brain configuration dynamic. Add `grug-config` tool for runtime brain management and lazy-reload `brains.json` on every tool call so external edits take effect without restart.

**Scope:**
- IN: `reloadBrains()` function that re-reads brains.json (with mtime cache to avoid unnecessary reads), called at the start of every tool handler. `grug-config` tool with three actions: `list` (show all brains with status), `add` (name, dir, options — writes to brains.json, creates dir if needed, starts sync timer if git remote provided), `remove` (name — removes from brains.json, stops sync timer, does NOT delete files). Update all tool handlers to use `getBrains()` helper that triggers reload.
- OUT: Ingest changes (Phase 2), doc refresh (Phase 3)

**Constraints:**
- Mtime-based cache: only re-parse brains.json when file mtime changes
- `grug-config add` must validate the same rules as `loadBrains()` (unique names, etc.)
- `grug-config remove` must not remove the primary brain
- New brain with git remote: start a sync timer immediately (don't wait for restart)
- Removing a brain with an active timer: clear the interval

**Approach notes:**
- User decided: MCP tool, not slash command. LLMs need programmatic brain management.

**File hints:**
- `server.js` — loadBrains() function, tool definitions section

**Depends on:** None | **Unlocks:** Phase 2, Phase 3

**Done when:**
- [ ] `brains.json` edits take effect on next tool call without restart
- [ ] `grug-config list` shows all brains with writable/read-only/git-synced status
- [ ] `grug-config add` creates brain entry, starts sync timer if git remote
- [ ] `grug-config remove` removes entry, stops timer, preserves files
- [ ] Cannot remove primary brain

**Difficulty:** MEDIUM
**Uncertainty:** Timer lifecycle — stopping/starting intervals for dynamically added brains

---

### Phase 2: Ingest Resilience
**Model:** sonnet

**Goal:** Make `/ingest` survive plugin updates by defaulting docs to `~/.grug-brain/<name>/` and storing the git source URL in brains.json so docs can be refreshed later.

**Scope:**
- IN: Update `/ingest` command to default target dir to `~/.grug-brain/<name>/` instead of `${CLAUDE_PLUGIN_ROOT}/docs/<name>/`. Store the source URL (GitHub or local path) in brains.json as a `source` field on the brain entry. When re-ingesting an existing brain, use the stored source if no new source provided. Update `/setup` to no longer reference plugin-relative docs paths.
- OUT: Auto-refresh (Phase 3)

**Constraints:**
- `~/.grug-brain/<name>/` is the default, user can override per-ingest
- `source` field in brains.json: `"source": "github:owner/repo/path"` or `"source": "/local/path"`
- Re-ingest with no args and existing source: pull latest from stored source

**Approach notes:**
- User decided: `~/.grug-brain/<name>/` is the centralized default location

**File hints:**
- `commands/ingest.md` — ingest command
- `commands/setup.md` — setup command

**Depends on:** Phase 1 | **Unlocks:** Phase 3

**Done when:**
- [ ] `/ingest` defaults to `~/.grug-brain/<name>/`
- [ ] Brain entry includes `source` field with origin URL/path
- [ ] Re-ingesting existing brain without source arg uses stored source
- [ ] `/setup` references `~/.grug-brain/` not plugin cache

**Difficulty:** LOW
**Uncertainty:** None

---

### Phase 3: Doc Auto-Refresh
**Model:** sonnet

**Goal:** Allow read-only doc brains to periodically refresh from their upstream source via an optional `refreshInterval` in brains.json.

**Scope:**
- IN: New `refreshInterval` field in brain config (seconds, default: none/disabled). On startup and at each interval: if brain has both `source` and `refreshInterval`, run a lightweight update (git pull for git-based sources, rsync for local). Reindex the brain after refresh. The refresh uses the same shallow-clone + rsync pattern as `/ingest` but runs automatically. Refresh only runs for read-only brains (writable brains use git sync instead). Log refresh activity to stderr.
- OUT: Nothing — final phase

**Constraints:**
- Refresh is read-only: pull/copy only, never push
- Minimum interval: 3600 seconds (1 hour) to avoid hammering upstream
- Git sources: `git -C <dir> pull --ff-only` if the dir is already a git repo, otherwise shallow clone + rsync
- Local sources: rsync from stored source path
- If refresh fails (network, permissions): log warning, retry at next interval

**Approach notes:**
- User decided: both manual (/ingest) and auto-refresh supported. refreshInterval is opt-in.

**File hints:**
- `server.js` — sync timer section, loadBrains config schema

**Depends on:** Phase 2 | **Unlocks:** None

**Done when:**
- [ ] Brain with `refreshInterval: 86400` refreshes docs daily
- [ ] Refresh reindexes the brain after pulling new content
- [ ] Minimum 1-hour interval enforced
- [ ] Refresh failures logged, don't crash server
- [ ] Brains without `refreshInterval` are not affected

**Difficulty:** MEDIUM
**Uncertainty:** Git pull vs fresh clone decision for updates — ff-only may fail if upstream rebased

---

## Test Coverage

**Level:** Automated (bun:test)

## Test Plan

- [ ] Unit: mtime-based config reload only re-reads when file changes
- [ ] Unit: grug-config add creates valid brain entry
- [ ] Unit: grug-config add rejects duplicate names
- [ ] Unit: grug-config remove prevents primary brain removal
- [ ] Unit: grug-config list shows all brains with status
- [ ] Integration: add brain via grug-config, immediately searchable
- [ ] Integration: remove brain via grug-config, timer stops
- [ ] Unit: ingest default dir is ~/.grug-brain/<name>/
- [ ] Unit: brain entry includes source field after ingest
- [ ] Integration: refreshInterval triggers periodic update

---

## Assumptions

| Assumption | Confidence | Verify Before Phase | Fallback If Wrong |
|-----------|-----------|--------------------|--------------------|
| clearInterval works on timers started in same process | HIGH | Phase 1 | Track timer IDs in a Map |
| git pull --ff-only is safe for read-only clones | MED | Phase 3 | Fall back to fresh shallow clone + rsync |
| brains.json mtime changes on every write | HIGH | Phase 1 | Use content hash instead |

## Decision Log

| Decision | Alternatives Considered | Rationale | Phase |
|----------|------------------------|-----------|-------|
| Lazy reload with mtime cache | File watcher, polling interval | Simplest reliable approach, no platform quirks | 1 |
| MCP tool for config | Slash command only | LLMs need programmatic access | 1 |
| ~/.grug-brain/<name>/ default | Plugin cache, always ask | Survives updates, centralized | 2 |
| Optional refreshInterval | Always auto-refresh, manual only | User controls complexity — opt-in | 3 |

---

## Notes

- The `grug-config add` action effectively replaces the "add more brains" step in `/setup`. Setup can delegate to it.
- Refresh and sync are different: sync is bidirectional (push+pull) for writable brains. Refresh is pull-only for read-only brains.
- Timer IDs should be stored in a Map keyed by brain name so they can be cleared on remove.

---

## Execution Log

_To be filled during /code-foundations:building_
