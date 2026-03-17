# grug-brain

Persistent memory for LLMs that works across projects. One MCP tool, six actions. Stores memories as markdown files, indexes them in SQLite FTS5 for ranked search. Scales to millions of documents.

## Why this instead of built-in memory?

Claude Code's `MEMORY.md` loads into every conversation whether you need it or not. Ten memories about your auth rewrite sitting there burning tokens while you're debugging CSS.

grug-brain keeps everything on disk until you ask for it. Memories live in one place, work across every project, and you only pull what's relevant. The rest costs nothing.

## Search that actually works

Under the hood, every memory gets indexed in a SQLite FTS5 virtual table with porter stemming and BM25 relevance scoring. That's the same ranking algorithm behind most search engines you've used.

What this means in practice: `run` finds `running`. `power` finds `powersync`. Multi-word queries match any term and rank by relevance, not insertion order. Twenty results per page.

But the real point is that the index is general-purpose. An LLM can throw anything at it: "how did we fix the auth bug," "what's the PowerSync gotcha with PATCH ops," "feedback about testing." Debugging notes, architectural decisions, deployment history, API gotchas, correction patterns. If you stored it, the LLM can find it with a natural language query and get back ranked results with highlighted snippets.

The index syncs incrementally on startup. Only files whose mtime changed get re-indexed. Cold start with thousands of memories takes milliseconds.

## Setup

```bash
npm install
```

Add to `~/.claude.json`:

```json
{
  "mcpServers": {
    "grug-brain": {
      "command": "node",
      "args": ["/path/to/grug-brain.mcp/server.js"]
    }
  }
}
```

Restart Claude Code.

## Actions

Call `grug-brain` with no arguments for usage. Each action returns its own help when you leave out a required param.

| Action | What it does |
|--------|-------------|
| `topics` | List categories and memory counts |
| `search` | BM25-ranked full-text search, 20 results per page |
| `read` | Read a memory file, 50 lines per page |
| `write` | Store a memory |
| `delete` | Remove a memory |
| `recall` | Preview all memories + dump full listing to a file. Filter by project. |

### write

`project` is the primary param. Files land in a folder named after the project:

```
action: "write"
project: "api-server"
text: "don't mock the database in integration tests"
name: "no-db-mocks"        # optional
```

Creates `memories/api-server/no-db-mocks.md`:

```markdown
---
name: no-db-mocks
date: 2026-03-16
type: memory
project: api-server
---

don't mock the database in integration tests
```

Some memories cut across projects. Feedback about `api-server` belongs in a shared `feedback` folder, not buried in the project directory. Use `target` to override the folder:

```
action: "write"
project: "api-server"
target: "feedback"
text: "don't mock the database in integration tests"
name: "no-db-mocks"
```

Creates `memories/feedback/no-db-mocks.md` with `project: api-server` in frontmatter. `recall project:"api-server"` catches both.

### search

```
action: "search"
text: "database mock"
```

Returns ranked results with highlighted snippets. Each result includes the file path (for follow-up reads), date, project, and category.

### recall

```
action: "recall"
project: "api-server"    # optional
```

Writes a full listing to `memories/recall.md` and returns the path plus a preview showing the two most recent entries per category. One call to catch up on a project.

### Custom frontmatter

Start your text with `---` and grug-brain keeps your frontmatter as-is.

## File layout

```
memories/
  .grug-brain.db        # SQLite FTS5 index, auto-managed
  api-server/
    no-db-mocks.md
    auth-rewrite.md
  feedback/
    no-summaries.md     # project: api-server
  react-native/
    keyboard-bug.md     # project: my-app
```

Folders default to the project name. `target` overrides the folder for cross-cutting memories. The `.grug-brain.db` file is the FTS5 index. Don't check it into version control.

## Environment

`MEMORY_DIR` controls where memories live. Defaults to `./memories/`.

## License

MIT
