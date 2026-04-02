---
description: Install and verify grug-brain. Checks binary, installs service, configures brains.
allowed-tools: Bash, Read, Write
---

Run these checks in order. Fix problems as you find them. Report a summary at the end.

## 0. Update check

If the server is already running (socket exists and is connectable), this is an **update**, not a fresh install.

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "RUNNING" || echo "NOT_RUNNING"
```

**If RUNNING** — skip to the fast update path:

1. Stop the existing service:
   - macOS: `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.server.plist 2>/dev/null || true`
   - Linux: `systemctl --user stop grug-brain.service 2>/dev/null || true`
2. Wait 2 seconds for the socket to be released
3. Re-install the service: `grug serve --install-service`
4. Wait 2 seconds, verify socket exists again
5. Skip to **step 6 (brains.json)**

**If NOT_RUNNING** — continue with full setup below.

## 1. Binary check

`grug --version` must work. If it doesn't, tell the user to install:

```bash
brew install rtd/grug/grug-brain
```

Verify after install:

```bash
grug --version
```

## 2. Service installation

Install grug-brain as a persistent background service. This creates and loads a launchd plist (macOS) or systemd unit (Linux).

### Kill stale processes

Previous sessions may have left zombie processes. Kill them:

```bash
pkill -f 'grug.*serve' 2>/dev/null || true
sleep 1
```

### Install service

```bash
grug serve --install-service
```

This writes the service file and loads it. The binary handles platform detection (macOS vs Linux) automatically.

### Verify

Wait 2 seconds, then check:

```bash
sleep 2
```

- macOS: `launchctl list | grep grug`
- Linux: `systemctl --user is-active grug-brain.service`

Also check the socket file exists:

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "Socket ready" || echo "Socket not found — check logs"
```

If the socket isn't ready, check logs:
- macOS: `~/.grug-brain/launchd-stderr.log`
- Linux: `journalctl --user -u grug-brain.service`

## 3. Server health

Test a tool call through the socket:

```bash
echo '{"id":"health","tool":"grug-search","params":{"query":"test"}}' | socat - UNIX-CONNECT:$HOME/.grug-brain/grug.sock 2>/dev/null
```

If `socat` is not available, the socket file existing and the service being listed is sufficient.

## 4. MCP registration

The plugin handles MCP registration automatically via `plugin.json`. Verify:

```bash
claude mcp list 2>/dev/null | grep -i grug
```

**If registered with old bun/HTTP reference**: remove and let the plugin re-register:

```bash
claude mcp remove grug-brain 2>/dev/null
```

Then tell the user to restart Claude Code so the plugin re-registers.

**If already showing `grug --stdio`**: no action needed.

## 5. Clean up old installation

If there are remnants of the old bun-based installation:

```bash
pkill -f 'bun.*grug-brain' 2>/dev/null || true
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist 2>/dev/null || true
rm -f ~/Library/LaunchAgents/com.grug-brain.mcp.plist 2>/dev/null || true
```

## 6. brains.json

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

## 7. Git setup

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

## 8. Summary

Report:
- grug version
- Service: installed + running / failed (with log path)
- MCP server: registered via plugin / needs restart
- Each brain: name, dir, file count, writable, git-synced
- Conflict count (if any)
- Tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-config, grug-dream

Service management:
- **macOS**: restart `launchctl kickstart -k gui/$(id -u)/com.grug-brain.server`, stop `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.server.plist`, logs `~/.grug-brain/launchd-stderr.log`
- **Linux**: restart `systemctl --user restart grug-brain.service`, stop `systemctl --user stop grug-brain.service`, logs `journalctl --user -u grug-brain.service`

Tell the user to restart Claude Code if the MCP server registration changed.
