# Plan: Unified Brain System

**Created:** 2026-03-26
**Status:** ready
**Complexity:** complex

---

## Context

Unify grug-brain's separate memory and docs systems into a single "brain" abstraction. Each brain is a directory of categorized markdown files with optional git sync. One unified FTS5 database indexes all brains. `grug-write` targets any writable brain. Conflict resolution saves local versions to a `conflicts/` category on rebase failure. Brains configured via `~/.grug-brain/brains.json`. Read-only brains block writes. Backwards compatible with existing `MEMORY_DIR`/`DOCS_DIRS` env vars.

## Constraints

- Single-file server (`server.js`)
- Backwards compatible with existing env vars (zero-config migration)
- Read-only brains must block writes with clear error
- Conflict files are writable, resolvable through existing grug tools
- No concurrent modification issues between sync timers and dream
- Network failures must be distinguished from rebase conflicts
- Sync lock prevents overlapping git operations per brain

## Chosen Approach

**Incremental Refactor**

Refactor server.js in place, phase by phase. Each phase produces a working server. Config first, then DB, then git, then tools, then dream/docs.

**Fallback:** If intermediate states cause too much churn, collapse remaining phases into a single rewrite of the affected section.

## Rejected Approaches

- **Clean Rewrite:** Higher risk — entire server non-functional until complete. Easy to miss features during port.
- **Extract-and-Replace:** Single-file constraint means "extraction" is just reorganizing within the file. No meaningful benefit over incremental.

---

## Implementation Phases

### Phase 1: Config Loading and Brain Abstraction
**Model:** sonnet
**Skills:** `code-foundations:cc-defensive-programming`

**Goal:** Replace `MEMORY_DIR` and `DOCS_ENTRIES` with a unified brain config loaded from `~/.grug-brain/brains.json`, with backwards-compatible fallback to env vars.

**Scope:**
- IN: `brains.json` schema, `loadBrains()` function, backwards-compat bridge from `MEMORY_DIR`/`DOCS_DIRS`, path resolution with `~` expansion, validation (exactly one primary, unique names)
- OUT: Database changes, tool changes, git changes

**Constraints:**
- Config at `~/.grug-brain/brains.json`. `GRUG_CONFIG` env var overrides.
- If no config file exists, build brains array from env vars (zero-config for existing users)
- Read-only brains: `writable: false` default for `flat: true` brains

**Approach notes:**
- `flat: true` brains default to `writable: false` (third-party docs are read-only unless explicitly marked writable)

**File hints:**
- `server.js` lines 14-32 — current config parsing

**Depends on:** None | **Unlocks:** Phase 2, Phase 3

**Done when:**
- [ ] `loadBrains()` returns array of brain objects from `brains.json` or env vars
- [ ] Validation rejects duplicate names, missing primary
- [ ] `~` expansion and `existsSync` filtering work
- [ ] Server starts successfully with either config source

**Difficulty:** MEDIUM
**Uncertainty:** None

---

### Phase 2: Unified Database
**Model:** sonnet
**Skills:** `code-foundations:aposd-simplifying-complexity`

**Goal:** Replace the two separate databases (`memDb` + `docsDb`) with a single FTS5 database at `~/.grug-brain/grug.db` that indexes all brains.

**Scope:**
- IN: Unified `brain_fts` table with `brain` column, unified `files` table with `(brain, path)` key, single `syncBrain()` function, schema migration (version bump forces reindex), merge `walkMemoryFiles`/`walkDocFiles` into one walker
- OUT: Tool changes (still use old stmts interface via adapter), git changes

**Constraints:**
- Schema version bump to 5 (drops and recreates tables, triggers full reindex)
- DB location: `~/.grug-brain/grug.db` (not inside any brain directory)
- `category` for flat brains = brain name
- Old DB files left as orphans (already gitignored)

**File hints:**
- `server.js` lines 226-478 — both DB sections

**Depends on:** Phase 1 | **Unlocks:** Phase 4

**Done when:**
- [ ] Single database at `~/.grug-brain/grug.db`
- [ ] All brains indexed in one `brain_fts` table
- [ ] `syncBrain()` handles both flat and category-based brains
- [ ] Search queries return results with `brain` field
- [ ] Startup log shows all brains and their file counts

**Difficulty:** MEDIUM
**Uncertainty:** None

---

### Phase 3: Per-Brain Git with Conflict Resolution
**Model:** opus
**Skills:** `code-foundations:cc-defensive-programming`, `code-foundations:cc-control-flow-quality`

**Goal:** Refactor git helpers from hardcoded `MEMORY_DIR` to per-brain, with independent sync timers and conflict resolution that saves local versions to a `conflicts/` category.

**Scope:**
- IN: `git(brain, ...args)` with brain-specific cwd, per-brain `ensureGitRepo`/`hasRemote`/`gitCommitFile`/`gitSync`, per-brain `setInterval`, conflict detection (check for `.git/REBASE_HEAD` after failed pull), conflict resolution (save local content to `conflicts/` category in primary brain with frontmatter), abort rebase + reset to remote, per-brain sync lock, hostname sanitization, `syncGitExclude` and `isLocalMemory` per-brain
- OUT: Dream changes (Phase 5), tool parameter changes (Phase 4)

**Constraints:**
- Network failure detection: pull returns null AND no `.git/REBASE_HEAD` means network error — skip, retry next interval
- Rebase conflict detection: `.git/REBASE_HEAD` exists after pull — trigger conflict resolution
- Conflict files go to `conflicts/` category in the primary brain
- Conflict frontmatter: `conflict: true`, `original_path`, `original_brain`, `hostname`, `date`
- Hostname sanitization: first segment, alphanumeric + hyphens only
- Sync lock: boolean flag per brain, checked by both `gitSync` and dream's git operations
- If conflict save fails (disk full, etc.): log to stderr, leave repo in rebase state for manual resolution

**Approach notes:**
- Conflict strategy chosen via 4-agent analysis: save to conflicts/ category (not git branches, not last-write-wins, not append-only)
- JS is single-threaded but setInterval callbacks interleave with tool handlers — sync lock is a simple boolean, not a mutex

**File hints:**
- `server.js` lines 68-153 — git section
- `server.js` lines 893-901 — sync timer

**Depends on:** Phase 1 | **Unlocks:** Phase 5

**Done when:**
- [ ] Git helpers accept brain object, use `brain.dir` as cwd
- [ ] Each brain with `git` remote gets independent sync timer using `brain.syncInterval`
- [ ] Network failures detected (no `REBASE_HEAD`) and skipped silently
- [ ] Rebase conflicts detected, local version saved to `conflicts/` with frontmatter
- [ ] After conflict save, rebase aborted, main reset to remote, FTS resynced
- [ ] Sync lock prevents overlapping git operations per brain

**Difficulty:** HIGH
**Uncertainty:** Edge case: conflict save itself fails. Fallback: log to stderr, leave repo in rebase state.

---

### Phase 4: Unified Tools
**Model:** sonnet
**Skills:** `code-foundations:cc-refactoring-guidance`

**Goal:** Update all 7 tools to use the unified brain system. Add `brain` parameter to write/read/delete/recall. Merge `grug-docs` into `grug-read`. Update `grug-search` to use unified FTS.

**Scope:**
- IN: Add optional `brain` param to grug-write, grug-read, grug-delete, grug-recall. Merge grug-docs browsing into grug-read (no args = list brains, brain only = list categories). Update grug-search to query unified table with brain tag in results. Block writes to read-only brains. Keep `grug-docs` as deprecated alias.
- OUT: Dream changes (Phase 5)

**Constraints:**
- `grug-write` defaults to primary brain
- Write to read-only brain returns: `brain "drizzle" is read-only`
- `grug-read` with no args lists all brains with status
- Search results include `[brain]` tag

**File hints:**
- `server.js` lines 535-900 — all tool definitions

**Depends on:** Phase 2 | **Unlocks:** Phase 5

**Done when:**
- [ ] `grug-write` accepts `brain` param, defaults to primary, blocks read-only
- [ ] `grug-read` lists brains (no args), categories (brain), files (brain+category), content (brain+category+path)
- [ ] `grug-search` returns results with brain field
- [ ] `grug-delete` accepts `brain` param, blocks read-only
- [ ] `grug-recall` accepts `brain` param
- [ ] `grug-docs` kept as alias

**Difficulty:** MEDIUM
**Uncertainty:** None

---

### Phase 5: Dream, Commands, and Documentation
**Model:** sonnet

**Goal:** Update dream to operate across all writable brains, surface conflicts. Update setup/ingest commands. Update README and bump to v3.0.0.

**Scope:**
- IN: Dream iterates all writable brains with git. Dream conflict section lists `conflicts/` entries with resolution guidance. Update `commands/setup.md` for brains.json management. Update `commands/ingest.md` to create brain entries. Update README. Bump plugin.json to 3.0.0. Update .gitignore.
- OUT: Nothing — final phase

**Constraints:**
- Dream must acquire sync lock before git operations
- Setup creates `brains.json` interactively if absent

**File hints:**
- `server.js` lines 735-846 — dream tool
- `commands/setup.md`, `commands/ingest.md`
- `README.md`, `.claude-plugin/plugin.json`

**Depends on:** Phase 3, Phase 4 | **Unlocks:** None

**Done when:**
- [ ] Dream commits pending changes per writable brain
- [ ] Dream surfaces `conflicts/` entries with resolution guidance
- [ ] Dream uses sync lock
- [ ] `/setup` creates/manages `brains.json`
- [ ] `/ingest` adds brain entries to config
- [ ] README documents `brains.json` format
- [ ] Version bumped to 3.0.0

**Difficulty:** MEDIUM
**Uncertainty:** Setup UX for brain management

---

## Test Coverage

**Level:** Per-phase manual verification

## Test Plan

- [ ] Server starts with `brains.json` config
- [ ] Server starts with env vars only (backwards compat)
- [ ] `grug-search` returns results from multiple brains
- [ ] `grug-write` to primary brain works
- [ ] `grug-write` to named writable brain works
- [ ] `grug-write` to read-only brain returns error
- [ ] `grug-read` with no args lists all brains
- [ ] Simulate rebase conflict: verify conflict file created in `conflicts/` category
- [ ] Verify network failure doesn't trigger conflict resolution
- [ ] Dream surfaces conflict files
- [ ] Sync lock prevents concurrent git operations

---

## Assumptions

| Assumption | Confidence | Verify Before Phase | Fallback If Wrong |
|-----------|-----------|--------------------|--------------------|
| `.git/REBASE_HEAD` reliably indicates rebase conflict | HIGH | Phase 3 | Use exit code parsing instead |
| Single-threaded JS means sync lock is a boolean | HIGH | Phase 3 | Use async mutex if needed |
| Schema version bump triggers clean reindex | HIGH | Phase 2 | Manual DB deletion |
| Flat brain dirs contain .md/.mdx files directly | HIGH | Phase 2 | Add recursive walk option |

## Decision Log

| Decision | Alternatives Considered | Rationale | Phase |
|----------|------------------------|-----------|-------|
| Conflict → save to conflicts/ category | Last-write-wins, keep-both, append-only, conflict branches | 4-agent analysis: only approach that works at the right layer (memories, not git) for the actual users (LLMs) | 3 |
| brains.json at ~/.grug-brain/ | Env var only, plugin dir | Survives plugin updates, simple fixed location | 1 |
| Incremental refactor | Clean rewrite, extract-replace | Each phase independently testable, lowest risk | All |
| Read-only default for flat brains | All writable | User decided: read-only repos should block writes | 1 |
| Unified DB at ~/.grug-brain/grug.db | Per-brain DBs, DB in primary brain | Neutral location, single source of truth for search | 2 |

---

## Notes

- The `cross_links` table in the current dream tool stores paths. These will need `brain:path` prefixing or a brain column. Pre-gate should discover the exact schema.
- The `recall.md` file is currently written to MEMORY_DIR. Under unified brains it should go to the primary brain's directory.
- Old `.grug-brain.db` and `.docs.db` files will be orphaned after migration. Consider adding a startup log noting they can be deleted.

---

## Execution Log

_To be filled during /code-foundations:building_
