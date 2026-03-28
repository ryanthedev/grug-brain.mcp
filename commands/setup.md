---
description: Install and verify grug-brain. Registers MCP server, checks dependencies, configures brains.
allowed-tools: Bash, Read
---

Run these checks in order. Fix problems as you find them. Report a summary at the end.

## 1. Bun runtime

`bun --version` must work. If it doesn't, tell the user to install Bun: `curl -fsSL https://bun.sh/install | bash`

## 2. Dependencies

`bun install` in `${CLAUDE_PLUGIN_ROOT}`.

## 3. MCP server registration

```bash
claude mcp list 2>/dev/null | grep -i grug
```

Not registered? Add it:

```bash
claude mcp add grug-brain -- bun run ${CLAUDE_PLUGIN_ROOT}/server.js
```

Already registered but pointing to a stale path? Remove and re-add.

## 4. Server health

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1.0"}}}' | timeout 5 bun run ${CLAUDE_PLUGIN_ROOT}/server.js 2>/dev/null
```

Valid JSON with `serverInfo` means it's working.

## 5. brains.json

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

### No config file

Create one interactively:

1. **self brain** (primary, local-only):
   - Dir: `~/.grug-brain/self` (default)
   - Primary brain. Writes land here. No git.

2. Ask: "Want a shared brain that syncs across machines?"
   - Name: "hive" (default). Git remote URL required.
   - Dir: `~/.grug-brain/memories` (default)
   - writable: true, syncInterval: 60

3. Ask: "Any other brains? Docs repos, reference material?"
   - Get name, directory, flat or not.
   - Typically read-only.

```bash
mkdir -p ~/.grug-brain
```

Write the JSON array to `~/.grug-brain/brains.json`.

### Config file exists

Show current brains. Ask: "Want to add another? You can also use `grug-config` anytime."

## 6. Git setup

For each brain with a `git` field, check initialization:

```bash
cd "<brain-dir>" && git rev-parse --git-dir 2>/dev/null
```

Not initialized? Set it up:

```bash
cd "<brain-dir>"
git init 2>/dev/null
git remote add origin <remote-url> 2>/dev/null || true
git pull origin main --rebase 2>/dev/null || git pull origin master --rebase 2>/dev/null || true
git add -A && git commit -m "grug: initial sync" --quiet 2>/dev/null || true
git push -u origin main 2>/dev/null || git push -u origin master 2>/dev/null || true
```

## 7. Summary

Report:
- Bun version (or missing)
- Dependencies: installed / error
- MCP server: registered + healthy / needs restart
- Each brain: name, dir, file count, writable, git-synced
- Conflict count (if any)
- Commands: `/dream`, `/setup`, `/ingest`
- Tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-config, grug-dream

Tell the user to restart Claude Code if the server registration changed.
