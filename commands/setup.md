---
description: Install and verify grug-brain. Registers MCP server, checks dependencies, installs OS service, configures brains.
allowed-tools: Bash, Read, Write
---

Run these checks in order. Fix problems as you find them. Report a summary at the end.

## 1. Bun runtime

`bun --version` must work. If it doesn't, tell the user to install Bun: `curl -fsSL https://bun.sh/install | bash`

Resolve the absolute path now — you need it later:

```bash
BUN_PATH=$(which bun)
```

## 2. Dependencies

`bun install` in `${CLAUDE_PLUGIN_ROOT}`.

## 3. Service installation

Install grug-brain as a persistent HTTP service so it survives across Claude Code sessions.

### Kill stale processes

Previous sessions may have left zombie stdio processes running (each keeps git sync and SQLite timers alive, causing contention). Kill them before installing the service:

```bash
pkill -f 'bun.*grug-brain.mcp/server.js' 2>/dev/null || true
pkill -f 'node.*grug-brain.mcp/server.js' 2>/dev/null || true
sleep 1
```

### Resolve paths

```bash
BUN_PATH=$(which bun)
SERVER_PATH="${CLAUDE_PLUGIN_ROOT}/server.js"
GRUG_PORT=${GRUG_PORT:-6483}
```

All three must be absolute paths / valid values. If `which bun` fails, stop.

### macOS (launchd)

Check: `[[ "$(uname)" == "Darwin" ]]`

Plist path: `~/Library/LaunchAgents/com.grug-brain.mcp.plist`

1. Stop any existing service:
   ```bash
   launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist 2>/dev/null || true
   ```

2. Ensure `~/.grug-brain` exists:
   ```bash
   mkdir -p ~/.grug-brain
   ```

3. Write the plist file to `~/Library/LaunchAgents/com.grug-brain.mcp.plist`.
   Use **Write tool** — do not use heredocs in bash.

   The plist must contain:
   - `Label`: `com.grug-brain.mcp`
   - `ProgramArguments`: array of `[$BUN_PATH, "run", "$SERVER_PATH"]` — use the resolved absolute paths, not variables
   - `WorkingDirectory`: the resolved `${CLAUDE_PLUGIN_ROOT}`
   - `KeepAlive`: true
   - `RunAtLoad`: true
   - `StandardOutPath`: the resolved `~/.grug-brain/launchd-stdout.log`
   - `StandardErrorPath`: the resolved `~/.grug-brain/launchd-stderr.log`
   - `EnvironmentVariables`: dict with `HOME` set to `$HOME` (resolved)
   - If `GRUG_PORT` is not 6483, add `GRUG_PORT` to the environment dict too

4. Load the service:
   ```bash
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist
   ```

5. Wait 2 seconds, then verify:
   ```bash
   curl -sf http://localhost:${GRUG_PORT}/mcp -X POST \
     -H "Content-Type: application/json" \
     -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1.0"}}}'
   ```
   Valid JSON with `serverInfo` means it's running. If it fails, tell the user to check `~/.grug-brain/launchd-stderr.log`.

### Linux (systemd)

Check: `[[ "$(uname)" == "Linux" ]]`

Unit path: `~/.config/systemd/user/grug-brain.service`

1. Ensure dirs exist:
   ```bash
   mkdir -p ~/.grug-brain ~/.config/systemd/user
   ```

2. Write the unit file to `~/.config/systemd/user/grug-brain.service`.
   Use **Write tool** — do not use heredocs in bash.

   Contents:
   ```ini
   [Unit]
   Description=grug-brain MCP server
   After=network.target

   [Service]
   Type=simple
   ExecStart=$BUN_PATH run $SERVER_PATH
   WorkingDirectory=${CLAUDE_PLUGIN_ROOT}
   Restart=always
   RestartSec=5
   Environment=HOME=$HOME
   Environment=GRUG_PORT=$GRUG_PORT

   [Install]
   WantedBy=default.target
   ```

   Use the resolved absolute paths, not variables.

3. Reload and enable:
   ```bash
   systemctl --user daemon-reload
   systemctl --user enable grug-brain.service
   systemctl --user restart grug-brain.service
   ```

4. Enable linger so the service survives logout:
   ```bash
   loginctl enable-linger $(whoami) 2>/dev/null || true
   ```

5. Wait 2 seconds, then verify:
   ```bash
   systemctl --user is-active grug-brain.service
   ```
   If it fails, tell the user to check: `journalctl --user -u grug-brain.service`

## 4. MCP server registration

```bash
claude mcp list 2>/dev/null | grep -i grug
```

**If registered as stdio** (output shows a `bun` command, not an HTTP URL): remove it first:

```bash
claude mcp remove grug-brain 2>/dev/null
```

**If not registered**, or was just removed, add as HTTP:

```bash
claude mcp add --transport http -s user grug-brain http://localhost:${GRUG_PORT}/mcp
```

**If already registered as HTTP** pointing to the right URL: no action needed.

## 5. Server health

```bash
curl -sf http://localhost:${GRUG_PORT:-6483}/mcp -X POST \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"setup","version":"1.0"}}}'
```

Valid JSON with `serverInfo` means it's working. If not, check:
- macOS: `~/.grug-brain/launchd-stderr.log`
- Linux: `journalctl --user -u grug-brain.service`

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
- Bun version (or missing)
- Dependencies: installed / error
- Service: installed + running / failed (with log path)
- MCP server: registered as HTTP / needs restart
- Each brain: name, dir, file count, writable, git-synced
- Conflict count (if any)
- Commands: `/dream`, `/setup`, `/ingest`
- Tools: grug-write, grug-search, grug-read, grug-recall, grug-delete, grug-config, grug-dream

Service management:
- **macOS**: restart `launchctl kickstart -k gui/$(id -u)/com.grug-brain.mcp`, stop `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist`, logs `~/.grug-brain/launchd-stderr.log`
- **Linux**: restart `systemctl --user restart grug-brain.service`, stop `systemctl --user stop grug-brain.service`, logs `journalctl --user -u grug-brain.service`

Tell the user to restart Claude Code if the server registration changed.
