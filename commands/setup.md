---
description: Install and verify grug-brain. Registers MCP server, checks dependencies, configures brains. Run after installing or updating.
allowed-tools: Bash, Read
---

Run these checks in order. Fix any issues found. Report a summary at the end.

## 1. Bun runtime

Verify `bun --version` works. grug-brain requires Bun for `bun:sqlite`. If missing, tell the user to install Bun: `curl -fsSL https://bun.sh/install | bash`

## 2. Dependencies

Run `bun install` in `${CLAUDE_PLUGIN_ROOT}` to ensure packages are up to date.

## 3. MCP server registration

Check if the grug-brain MCP server is already registered:

```bash
claude mcp list 2>/dev/null | grep -i grug
```

If NOT registered, register it:

```bash
claude mcp add grug-brain -- bun run ${CLAUDE_PLUGIN_ROOT}/server.js
```

If already registered, check that the command path points to `${CLAUDE_PLUGIN_ROOT}/server.js`. If stale, remove and re-add.

## 4. MCP server health

Smoke-test the server:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1.0"}}}' | timeout 5 bun run ${CLAUDE_PLUGIN_ROOT}/server.js 2>/dev/null
```

If this returns a valid JSON response with `serverInfo`, the server is healthy.

## 5. brains.json configuration

Check if `~/.grug-brain/brains.json` exists:

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

### No config file

Walk the user through creating one:

1. Create the self brain (primary, local-only):
   - Directory: `~/.grug-brain/self` (default, user can change)
   - This is the primary brain. Writes go here by default. No git sync.

2. Ask: "Do you want a shared brain that syncs across machines?"
   - If yes, ask for a name (default: "hive") and git remote URL
   - Directory: `~/.grug-brain/memories` (default)
   - Add with writable: true, syncInterval: 60

3. Ask: "Do you want to add any other brains? (docs repos, reference material)"
   - If yes, ask for name, directory, and whether it's flat or has category subdirectories
   - These are typically read-only

Write the config:

```bash
mkdir -p ~/.grug-brain
```

Write a JSON array to `~/.grug-brain/brains.json` with the configured brains.

### Config file exists

Show the current brains and ask: "Want to add another brain? You can also use `grug-config` at any time."

## 6. Git setup for synced brains

For each brain with a `git` field, check if git is initialized:

```bash
cd "<brain-dir>" && git rev-parse --git-dir 2>/dev/null
```

If not initialized and the brain has a git remote, set it up:

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
- Bun: version or missing
- Dependencies: installed / error
- MCP server: registered + healthy / needs restart
- Brains: list each brain (name, dir, file count, writable, git-synced)
- Conflicts: count if any entries in the `conflicts/` category
- Available commands: `/dream`, `/setup`, `/ingest`
- Available tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-config, grug-dream

If the MCP server was just registered or updated, tell the user to restart Claude Code.
