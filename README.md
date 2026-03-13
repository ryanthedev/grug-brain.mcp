# grug-brain

grug forget things. important things. things grug learned the hard way. then grug make same mistake again. very sad.

so grug build MCP server. one tool, six actions. grug write memories into markdown files with frontmatter. each category gets `llms.txt` index so grug can search without stuffing whole brain into context window.

## why not claude built-in memory

claude memory is per-project. always loaded. every memory sitting there eating up space whether grug need it or not.

grug-brain different. memories live in one place. work across all projects. only show up when grug ask. grug pull what grug need, rest stays on disk.

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

need `rg` (ripgrep) on PATH for search.

## usage

call `grug-brain` with no arguments. it tell you what to do. each action has own help too.

| Action | What it does |
|--------|-------------|
| `topics` | List all categories and memory counts |
| `search` | Find memories across all categories (regex supported) |
| `read` | Read a specific memory file (paginated) |
| `write` | Save a memory to a category |
| `delete` | Remove a memory |
| `build` | Rebuild a category's `llms.txt` index |

### write memory

```
action: "write"
target: "feedback"
text: "don't mock the database in integration tests"
name: "no-db-mocks"        # optional, derived from text if omitted
project: "api-server"      # optional
```

this make `memories/feedback/no-db-mocks.md`:

```markdown
---
name: no-db-mocks
date: 2026-03-12
type: memory
project: api-server
---

don't mock the database in integration tests
```

also rebuild `memories/feedback/llms.txt` so grug can find it later.

### search

```
action: "search"
text: "database"
```

ripgrep go through all `llms.txt` indexes. come back with category, line number, matching text.

### custom frontmatter

text start with `---`? grug-brain keep it as-is. write own frontmatter when grug want more control.

## how it work

```
memories/
  feedback/
    llms.txt          # auto-generated index
    no-db-mocks.md
    no-summaries.md
  decisions/
    llms.txt
    auth-rewrite.md
```

category is folder. memory is markdown file with YAML frontmatter. `llms.txt` is auto-generated index. one line per memory: title, date, project, description from file body. search hit indexes, not individual files.

grug only load what grug actually read.

## environment

set `MEMORY_DIR` to put memories somewhere else. default is `./memories/`.

## license

MIT
