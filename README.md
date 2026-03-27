# grug-brain

Persistent memory for LLMs that works across projects. Stores memories as markdown files, indexes them in SQLite FTS5 for ranked search. Supports any number of knowledge sources ("brains") — memory stores, doc libraries, notes directories — all unified in one searchable index. Git-tracked history with a dreaming feature for memory maintenance.

## Install

```bash
claude plugin add grug-brain
/setup
```

`/setup` handles everything: installs dependencies, registers the MCP server, verifies health, and walks you through creating `~/.grug-brain/brains.json`.

## Tools

| Tool | What it does |
|------|-------------|
| `grug-write` | Store a memory (category + path + markdown content) |
| `grug-search` | BM25-ranked full-text search across all brains |
| `grug-read` | Browse brains, list categories, list memories, read files |
| `grug-recall` | Get up to speed — preview + full listing to recall.md |
| `grug-delete` | Remove a memory |
| `grug-dream` | Review memory health across all brains: git history, cross-links, conflicts, stale detection |

## Brains

grug-brain treats every knowledge source as a "brain" — a directory of markdown files. All brains share one FTS5 index, so `grug-search` spans everything.

Configure brains in `~/.grug-brain/brains.json`:

```json
[
  {
    "name": "memories",
    "dir": "~/.grug-brain/memories",
    "primary": true,
    "writable": true,
    "git": "git@github.com:you/memories.git",
    "syncInterval": 60
  },
  {
    "name": "grug-docs",
    "dir": "/repos/grug-docs",
    "primary": false,
    "writable": false,
    "flat": false,
    "git": null
  },
  {
    "name": "drizzle",
    "dir": "/repos/drizzle-docs/content",
    "primary": false,
    "writable": false,
    "flat": true,
    "git": null
  }
]
```

### Brain fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | yes | — | Unique identifier used in tool calls |
| `dir` | yes | — | Directory path. Supports `~` expansion. |
| `primary` | yes | — | Exactly one brain must be `true`. Receives conflict files and recall.md. |
| `writable` | no | `true` (or `false` if flat) | Whether `grug-write` and `grug-delete` work here |
| `flat` | no | `false` | `true` = files directly in dir with no category subdirectories |
| `git` | no | `null` | Git remote URL for sync. `null` = local only. |
| `syncInterval` | no | `60` | How often to sync with git remote, in seconds |

### Brain types

**Memory brain** (`writable: true`, `flat: false`): The primary store for memories. Each subdirectory is a category. The primary brain also receives conflict files from git merges.

**Docs brain** (`writable: false`, `flat: false`): Read-only reference documentation. Each subdirectory is a category. Add via `/ingest`.

**Flat brain** (`flat: true`): Files live directly in the directory, no category subdirectories. The brain name becomes the category. Read-only by default.

### First run

If `~/.grug-brain/brains.json` doesn't exist, grug-brain creates a default config with a single primary brain at `~/.grug-brain/memories/`. Run `/setup` to add more brains.

## Dreaming

`/dream` reviews your memory store and takes maintenance actions across all brains:

- **Git history**: Commits pending changes per writable brain, shows recent changelog
- **Conflicts**: Lists entries in the `conflicts/` category with resolution guidance
- **Cross-links**: Finds related memories across different categories and brains
- **Stale detection**: Flags memories older than 90 days for review
- **Quality issues**: Catches missing dates or descriptions

Run once manually, or set up periodic maintenance:

```
/loop 30m /dream
```

### Conflict resolution

When a git rebase fails during sync, grug-brain saves your local version to the `conflicts/` category in the primary brain. Each conflict entry includes the original path, source brain, hostname, and date. The dream report lists all conflicts with step-by-step resolution guidance:

1. Read the conflict file with `grug-read`
2. Write the correct version to the original path with `grug-write`
3. Delete the conflict entry with `grug-delete`

## Search

SQLite FTS5 with porter stemming and BM25 ranking. `run` finds `running`. `power` finds `powersync`. Multi-word queries match any term and rank by relevance. 20 results per page with highlighted snippets. Results show which brain each file came from.

The index syncs incrementally on startup — only files whose mtime changed get re-indexed.

## File layout

```
~/.grug-brain/
  brains.json             # brain configuration
  grug.db                 # unified FTS5 index (auto-managed)
  memories/               # primary brain (default)
    api-server/
      no-db-mocks.md
    feedback/
      no-summaries.md
    conflicts/            # git conflict files (auto-managed)
      memories--api-server--no-db-mocks.md
```

## License

MIT
