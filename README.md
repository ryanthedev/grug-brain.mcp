# grug-brain

grug forget things. important things. things grug learned the hard way. then grug make same mistake again. very sad.

so grug build MCP server. one tool, six actions. grug write memories into markdown files with frontmatter. SQLite FTS5 index so grug can search with BM25 ranking without stuffing whole brain into context window.

## why not claude built-in memory

claude memory is per-project. always loaded. every memory sitting there eating up space whether grug need it or not.

grug-brain different. memories live in one place. work across all projects. only show up when grug ask. grug pull what grug need, rest stays on disk. scales to millions of small docs.

## setup

```bash
npm install
```

add to `~/.claude.json`:

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

restart claude code. `grug-brain` tool ready.

## usage

call `grug-brain` with no arguments. it tell you what to do. each action has own help too.

| Action | What it does |
|--------|-------------|
| `topics` | List all categories and memory counts |
| `search` | Find memories across all categories (FTS5 + BM25 ranked, paged) |
| `read` | Read a specific memory file (paginated) |
| `write` | Save a memory to a project |
| `delete` | Remove a memory |
| `recall` | Get up to speed — preview + full dump to file (optional project filter) |

### write memory

`project` is the primary param. file goes in a folder named after the project. use `target` to override the folder for cross-cutting memories (like feedback that applies to a specific project).

```
action: "write"
project: "api-server"
text: "don't mock the database in integration tests"
name: "no-db-mocks"        # optional, derived from text if omitted
```

this make `memories/api-server/no-db-mocks.md`:

```markdown
---
name: no-db-mocks
date: 2026-03-16
type: memory
project: api-server
---

don't mock the database in integration tests
```

cross-cutting memory (feedback about a project, filed under feedback):

```
action: "write"
project: "api-server"
target: "feedback"
text: "don't mock the database in integration tests"
name: "no-db-mocks"
```

this make `memories/feedback/no-db-mocks.md` with `project: api-server` in frontmatter. `recall project:"api-server"` find both.

### search

```
action: "search"
text: "database mock"
```

FTS5 full-text search with BM25 relevance ranking. porter stemming (`run` match `running`), prefix matching, paged results (20 per page). multi-word queries use OR (match any term). results show file path, date, project, category, and highlighted description snippet.

### recall

```
action: "recall"
project: "api-server"    # optional — omit to see everything
```

writes full memory listing to `memories/recall.md`, returns absolute file path + preview (2 most recent entries per category). one call to get up to speed on a project.

### custom frontmatter

text start with `---`? grug-brain keep it as-is. write own frontmatter when grug want more control.

## how it work

```
memories/
  .grug-brain.db        # SQLite FTS5 index (auto-managed)
  api-server/
    no-db-mocks.md
    auth-rewrite.md
  feedback/
    no-summaries.md     # project: api-server in frontmatter
  react-native/
    keyboard-bug.md     # project: my-app in frontmatter
```

project is the primary organizer. category folder defaults to project name. `target` overrides the folder for cross-cutting memories. SQLite FTS5 indexes all files on startup (incremental — only re-indexes changed files via mtime). search hits the FTS index, not individual files.

grug only load what grug actually read.

## environment

set `MEMORY_DIR` to put memories somewhere else. default is `./memories/`.

## license

MIT
