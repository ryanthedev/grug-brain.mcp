---
description: Install and verify grug-brain. Registers MCP server, checks dependencies, sets up dreaming cron. Run after cloning or updating.
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

If already registered, check that the command path is correct (points to `${CLAUDE_PLUGIN_ROOT}/server.js`). If the path is stale (old location), remove and re-add:

```bash
claude mcp remove grug-brain
claude mcp add grug-brain -- bun run ${CLAUDE_PLUGIN_ROOT}/server.js
```

## 4. MCP server health

Smoke-test the server by sending an initialize request:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1.0"}}}' | timeout 5 bun run ${CLAUDE_PLUGIN_ROOT}/server.js 2>/dev/null
```

If this returns a valid JSON response with `serverInfo`, the server is healthy. If it fails, read stderr for the error.

## 5. Memory git repo

Check if the memories directory has git initialized:

```bash
MEMORY_DIR="${MEMORY_DIR:-${CLAUDE_PLUGIN_ROOT}/memories}"
cd "$MEMORY_DIR" && git rev-parse --git-dir 2>/dev/null
```

If not a git repo, inform the user that `/dream` will auto-initialize it on first run.

## 6. Dream cron

Ask the user if they want to set up periodic dreaming. If yes, set up a cron that runs every 4 hours:

```bash
claude cron add --name "grug-dream" --schedule "0 */4 * * *" --command "/dream" 2>/dev/null
```

If `claude cron` is not available, tell the user they can use `/loop 30m /dream` during active sessions instead.

## 7. Summary

Report:
- Bun: version or missing
- Dependencies: installed / error
- MCP server: registered + healthy / needs restart
- Memories: count of `.md` files, git status
- Docs: count if docs/ exists, or "none"
- Dream cron: active / not set up
- Available commands: `/dream`, `/setup`
- Available tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-dream, grug-docs

If the MCP server was just registered or updated, tell the user to restart Claude Code for changes to take effect.
