# grug-brain

Persistent memory for LLMs that works across projects. Stores memories as markdown files, indexes them in SQLite FTS5 for ranked search. Optional bundled docs. Git-tracked history with a dreaming feature for memory maintenance.

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
| `grug-docs` | Browse/read bundled documentation (if docs/ exists) |

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
  .grug-brain.db        # FTS5 index (auto-managed, gitignored)
  api-server/
    no-db-mocks.md
  feedback/
    no-summaries.md
docs/                   # optional bundled documentation
  react-native/
    *.md
```

## Environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `MEMORY_DIR` | `./memories/` | Where memories live |
| `DOCS_DIR` | `./docs/` | Where bundled docs live |

## License

MIT
