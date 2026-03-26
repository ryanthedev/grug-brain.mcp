# grug-brain

Persistent memory for LLMs that works across projects. Stores memories as markdown files, indexes them in SQLite FTS5 for ranked search. Reads documentation from any number of source directories. Git-tracked history with a dreaming feature for memory maintenance.

## Install

```bash
claude plugin add grug-brain
/setup
```

`/setup` handles everything: installs dependencies, registers the MCP server, verifies health, and optionally sets up a dreaming cron.

## Tools

| Tool | What it does |
|------|-------------|
| `grug-write` | Store a memory (category + path + markdown content) |
| `grug-search` | BM25-ranked full-text search across memories and docs |
| `grug-read` | Browse categories, list memories, read files |
| `grug-recall` | Get up to speed — preview + full listing to recall.md |
| `grug-delete` | Remove a memory |
| `grug-dream` | Review memory health: git history, cross-links, stale detection |
| `grug-docs` | Browse and read documentation from all configured sources |

## Docs

Point `DOCS_DIRS` at any number of documentation directories. The server indexes every `.md` and `.mdx` file with FTS5 on startup, so `grug-search` and `grug-docs` cover both memories and docs.

Two formats, separated by colons:

```
# Each subdirectory becomes a category
/path/to/grug-docs

# The whole directory becomes one named category
drizzle=/path/to/drizzle-orm-docs/src/content/docs
```

Combine them in `DOCS_DIRS`:

```json
{
  "env": {
    "DOCS_DIRS": "/repos/grug-docs:drizzle=/repos/drizzle-docs:react-native=/repos/rn-docs"
  }
}
```

The first form works when the directory already contains category subdirectories (like a docs monorepo). The second form wraps a flat docs directory under a name you choose. Add or remove sources without touching any code — just update the env var and restart.

Category browsing is paginated. The FTS5 index lives at `memories/.docs.db` and syncs incrementally on startup.

## Dreaming

`/dream` reviews your memory store and takes maintenance actions:

- **Git history**: Commits pending memory changes, shows recent changelog
- **Cross-links**: Finds related memories across different categories
- **Stale detection**: Flags memories older than 90 days for review
- **Quality issues**: Catches missing dates or descriptions

Run once manually, or set up periodic maintenance:

```
/loop 30m /dream
```

## Search

SQLite FTS5 with porter stemming and BM25 ranking. `run` finds `running`. `power` finds `powersync`. Multi-word queries match any term and rank by relevance. 20 results per page with highlighted snippets.

The index syncs incrementally on startup — only files whose mtime changed get re-indexed.

## File layout

```
memories/
  .grug-brain.db        # memory FTS5 index (auto-managed, gitignored)
  .docs.db              # docs FTS5 index (auto-managed, gitignored)
  api-server/
    no-db-mocks.md
  feedback/
    no-summaries.md
```

## Environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `MEMORY_DIR` | `~/.grug-brain/memories/` | Where memories live (survives plugin updates) |
| `DOCS_DIRS` | `./docs/` | Colon-separated list of doc directories. Supports `name=path` for named categories. Also accepts `DOCS_DIR` for backwards compatibility. |

## License

MIT
