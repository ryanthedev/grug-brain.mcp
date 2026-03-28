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

## 5. brains.json configuration

Check if `~/.grug-brain/brains.json` exists:

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

If the file does NOT exist, create it interactively:

1. Tell the user: "No brains.json found. Let's create one. You need at least one primary brain for memories."
2. Ask: "Where should the primary memories brain live? (default: `~/.grug-brain/memories`)"
3. Ask: "Does this brain sync via git? If yes, provide the remote URL (e.g., `git@github.com:user/memories.git`). If no, press enter."
4. Create `~/.grug-brain/` if it doesn't exist:
   ```bash
   mkdir -p ~/.grug-brain
   ```
5. Write the initial `~/.grug-brain/brains.json`:
   ```json
   [
     {
       "name": "memories",
       "dir": "<chosen dir>",
       "primary": true,
       "writable": true,
       "git": "<remote url or null>"
     }
   ]
   ```

If the file DOES exist, show the current brains and ask: "Do you want to add another brain?"

### Adding additional brains

If the user wants to add a brain, ask for:

1. **Name** — short identifier (e.g., `grug-docs`, `work-notes`)
2. **Directory** — local path (e.g., `/repos/grug-docs`, `~/notes`)
3. **Type** — writable memory brain or read-only docs brain?
4. **Flat** — does the directory contain files directly (no category subdirectories)? Flat brains are read-only by default.
5. **Git remote** — URL or blank for local-only

Add the entry to `~/.grug-brain/brains.json`. Example for a read-only flat docs brain:

```json
{
  "name": "grug-docs",
  "dir": "/repos/grug-docs",
  "primary": false,
  "writable": false,
  "flat": false,
  "git": null
}
```

Repeat until the user says they're done.

## 6. Primary brain git setup

Check if the primary brain directory has git initialized:

```bash
PRIMARY_DIR=$(cat ~/.grug-brain/brains.json | python3 -c "import sys,json; brains=json.load(sys.stdin); print(next(b['dir'] for b in brains if b.get('primary')))" 2>/dev/null || echo "${HOME}/.grug-brain/memories")
cd "$PRIMARY_DIR" && git rev-parse --git-dir 2>/dev/null
```

If not a git repo, it will auto-initialize on the first `grug-write`.

If the primary brain has a `git` remote configured in brains.json but git is not yet set up, initialize and connect:

```bash
cd "$PRIMARY_DIR"
git init 2>/dev/null
git remote add origin <remote-url> 2>/dev/null || true
git pull origin main --rebase 2>/dev/null || git pull origin master --rebase 2>/dev/null || true
git add -A && git commit -m "grug: initial sync" --quiet 2>/dev/null || true
git push -u origin main 2>/dev/null || git push -u origin master 2>/dev/null || true
```

Tell the user: sync runs automatically every 60 seconds while the MCP server is running. Memories in the `local/` category are never synced (add `sync: false` to frontmatter to keep individual files local).

## 7. Summary

Report:
- Bun: version or missing
- Dependencies: installed / error
- MCP server: registered + healthy / needs restart
- Brains: list each brain (name, dir, file count, writable, git-synced)
- Conflicts: count if any entries in the `conflicts/` category
- Available commands: `/dream`, `/setup`, `/ingest`
- Available tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-dream

If the MCP server was just registered or updated, tell the user to restart Claude Code for changes to take effect.
