---
description: Install and verify grug-brain. Registers MCP server, checks dependencies, configures shared brain. Run after installing or updating.
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

If already registered, check that the command path is correct (points to `${CLAUDE_PLUGIN_ROOT}/server.js`). If the path is stale, remove and re-add:

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

If not a git repo, it will auto-initialize on the first `grug-write`.

## 6. Shared brain

Ask the user: **"Do you want to connect a shared brain? This syncs memories across machines via a git remote."**

If yes:

1. Ask for the remote repo URL (e.g., `https://github.com/user/grug-memories.git` or `git@github.com:user/grug-memories.git`)
2. Initialize the memory git repo if needed:
   ```bash
   MEMORY_DIR="${MEMORY_DIR:-${CLAUDE_PLUGIN_ROOT}/memories}"
   cd "$MEMORY_DIR"
   git init 2>/dev/null
   ```
3. Check if a remote already exists:
   ```bash
   cd "$MEMORY_DIR" && git remote -v
   ```
4. If no remote, add one:
   ```bash
   cd "$MEMORY_DIR" && git remote add origin <repo-url>
   ```
5. Do an initial pull (if the remote repo already has content):
   ```bash
   cd "$MEMORY_DIR" && git pull origin main --rebase 2>/dev/null || git pull origin master --rebase 2>/dev/null
   ```
6. Push current memories:
   ```bash
   cd "$MEMORY_DIR" && git add -A && git commit -m "grug: initial sync" --quiet 2>/dev/null; git push -u origin main 2>/dev/null || git push -u origin master 2>/dev/null
   ```

Tell the user: sync runs automatically every 60 seconds while the MCP server is running. Memories in the `local/` category are never synced.

If no, skip. Sync can be configured later by re-running `/setup`.

## 7. Summary

Report:
- Bun: version or missing
- Dependencies: installed / error
- MCP server: registered + healthy / needs restart
- Memories: count of `.md` files, git status
- Shared brain: connected (remote URL) / local only
- Docs: count if docs/ exists, or "none"
- Available commands: `/dream`, `/setup`, `/ingest`
- Available tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-dream, grug-docs

If the MCP server was just registered or updated, tell the user to restart Claude Code for changes to take effect.
