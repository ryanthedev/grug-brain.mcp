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

Re-read `brains.json` on tool calls to pick up external changes. `grug-config` tool provides add/remove/list actions and can start sync/refresh timers for newly added brains. `/ingest` updated to default to `~/.grug-brain/<name>/`. Optional `refreshInterval` field enables periodic pull for read-only doc brains.

**Fallback:** If lazy reload adds noticeable latency, add mtime-based caching (read only when file changed).

## Rejected Approaches

- **File watcher (fs.watch):** Unreliable cross-platform. Adds complexity for marginal benefit over lazy reload.
- **Slash command only:** LLMs can't manage brains mid-conversation without user invoking a command.

---

## Implementation Phases

### Phase 1: Runtime Brain Management
**Model:** sonnet

**Goal:** Make brain configuration fully dynamic — add `grug-config` tool, lazy-reload config, update `/ingest` and `/setup` commands to use stable paths, and support doc auto-refresh via `refreshInterval`.

**Scope:**
- IN: `reloadBrains()` with mtime cache called at start of every tool handler. `grug-config` tool with list/add/remove actions (add creates dir, writes brains.json, starts sync or refresh timer; remove stops timer, preserves files). Timer IDs tracked in a Map for lifecycle management. Update `/ingest` default target to `~/.grug-brain/<name>/`, store `source` field in brain entry. Update `/setup` to not reference `${CLAUDE_PLUGIN_ROOT}/docs/`. Add `refreshInterval` support: read-only brains with both `source` and `refreshInterval` get periodic pull (git or rsync), minimum 1 hour, writable brains skipped. Reindex brain after refresh.
- OUT: Nothing — single phase covers all gaps

**Constraints:**
- Mtime-based cache: only re-parse brains.json when file mtime changes
- `grug-config add` validates same rules as `loadBrains()` (unique names, etc.)
- `grug-config remove` must not remove the primary brain
- New brain with git remote: start sync timer immediately
- New brain with `refreshInterval`: start refresh timer immediately
- Removing a brain: clear its interval
- Refresh skips brains where `writable: true`
- Refresh minimum interval: 3600 seconds
- Git refresh: `git -C <dir> pull --ff-only`, fallback to shallow clone + rsync if ff fails
- Refresh failures: log warning, retry at next interval
- `source` field uses existing `github:owner/repo/path` syntax from `/ingest`
- `~/.grug-brain/<name>/` is the default ingest location, user can override

**Approach notes:**
- User decided: MCP tool, not slash command. LLMs need programmatic brain management.
- User decided: `~/.grug-brain/<name>/` is the centralized default location for ingested docs.
- User decided: both manual (/ingest) and auto-refresh supported. refreshInterval is opt-in.
- Refresh and sync are different: sync is bidirectional (push+pull) for writable brains. Refresh is pull-only for read-only brains.

**File hints:**
- `server.js` — loadBrains(), tool definitions, sync timer section
- `commands/ingest.md` — ingest command
- `commands/setup.md` — setup command

**Depends on:** None | **Unlocks:** Phase 2

**Done when:**
- [ ] `brains.json` edits take effect on next tool call without restart
- [ ] `grug-config list` shows all brains with writable/read-only/git-synced/refresh status
- [ ] `grug-config add` creates brain entry, starts appropriate timer
- [ ] `grug-config remove` removes entry, stops timer, preserves files
- [ ] Cannot remove primary brain
- [ ] `/ingest` target dir default no longer uses `${CLAUDE_PLUGIN_ROOT}`
- [ ] Brain entry includes `source` field using existing `github:owner/repo/path` syntax
- [ ] Re-ingesting existing brain without source arg uses stored source
- [ ] Brain with `refreshInterval: 86400` refreshes daily, new docs appear in `grug-search`
- [ ] Refresh failures logged, don't crash server
- [ ] A writable brain with `refreshInterval` set is not auto-refreshed

**Difficulty:** MEDIUM
**Uncertainty:** Timer lifecycle — stopping/starting intervals for dynamically added brains. Git ff-only may fail if upstream rebased.

---

### Phase 2: Validation
**Model:** sonnet

**Goal:** Automated test suite covering config reload, grug-config actions, ingest paths, and refresh behavior. Plus a manual walkthrough of the first-time user flow.

**Scope:**
- IN: bun:test suite for: mtime-based reload, grug-config add/remove/list, duplicate name rejection, primary brain protection, ingest default dir, source field storage, refreshInterval timer management. Manual verification: first-time `/setup` flow, adding a second brain, ingesting GitHub subfolder docs, verifying refresh works end-to-end.
- OUT: Nothing — final phase

**Constraints:**
- Tests must run without network access (mock git operations where needed)
- Manual walkthrough documents any UX issues for follow-up

**File hints:**
- `test-*.js` — test files
- `server.js` — functions under test

**Depends on:** Phase 1 | **Unlocks:** None

**Done when:**
- [ ] All unit tests pass: reload, config actions, validation, ingest defaults, refresh rules
- [ ] Manual walkthrough of first-time user flow documented
- [ ] Manual walkthrough of adding shared brain documented
- [ ] Manual walkthrough of ingesting GitHub subfolder docs documented

**Difficulty:** LOW
**Uncertainty:** None

---

## Test Coverage

**Level:** Automated (bun:test)

## Test Plan

- [ ] Unit: mtime-based config reload only re-reads when file changes
- [ ] Unit: grug-config add creates valid brain entry with correct defaults
- [ ] Unit: grug-config add rejects duplicate names
- [ ] Unit: grug-config remove prevents primary brain removal
- [ ] Unit: grug-config list shows all brains with status
- [ ] Unit: ingest default dir is ~/.grug-brain/<name>/
- [ ] Unit: brain entry includes source field after ingest
- [ ] Unit: refreshInterval only applies to read-only brains
- [ ] Integration: add brain via grug-config, immediately searchable
- [ ] Integration: remove brain via grug-config, timer stops
- [ ] Integration: refreshInterval triggers periodic update, new docs searchable
- [ ] Manual: first-time user /setup flow
- [ ] Manual: add shared brain with git remote
- [ ] Manual: ingest GitHub subfolder docs

---

## Assumptions

| Assumption | Confidence | Verify Before Phase | Fallback If Wrong |
|-----------|-----------|--------------------|--------------------|
| clearInterval works on timers started in same process | HIGH | Phase 1 | Track timer IDs in a Map |
| git pull --ff-only is safe for read-only clones | MED | Phase 1 | Fall back to fresh shallow clone + rsync |
| brains.json mtime changes on every write | HIGH | Phase 1 | Use content hash instead |

## Decision Log

| Decision | Alternatives Considered | Rationale | Phase |
|----------|------------------------|-----------|-------|
| Lazy reload with mtime cache | File watcher, polling interval | Simplest reliable approach, no platform quirks | 1 |
| MCP tool for config | Slash command only | LLMs need programmatic access | 1 |
| ~/.grug-brain/<name>/ default | Plugin cache, always ask | Survives updates, centralized | 1 |
| Optional refreshInterval | Always auto-refresh, manual only | User controls complexity — opt-in | 1 |

---

## Notes

- The `grug-config add` action effectively replaces the "add more brains" step in `/setup`. Setup can delegate to it.
- Timer IDs should be stored in a Map keyed by brain name so they can be cleared on remove.
- Refresh timer and sync timer are mutually exclusive per brain: writable brains get sync, read-only brains can get refresh.

---

## Execution Log

_To be filled during /code-foundations:building_
