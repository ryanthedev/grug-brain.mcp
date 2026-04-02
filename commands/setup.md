---
description: Install and verify grug-brain. Builds from source, installs binary, sets up service, configures brains.
allowed-tools: Bash, Read, Write, AskUserQuestion
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
3. Build the new binary (step 1 below)
4. Re-install the service: `~/.grug-brain/bin/grug serve --install-service`
5. Wait 2 seconds, verify socket exists again
6. Skip to **step 6 (brains.json)**

**If NOT_RUNNING** — continue with full setup below.

## 1. Rust toolchain

`cargo --version` must work. If it doesn't:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

Verify after install:

```bash
cargo --version
```

## 2. Build from source

Build the release binary from the plugin root:

```bash
cd "${CLAUDE_PLUGIN_ROOT}"
cargo build --release
```

This takes ~30s on first build. The binary lands at `${CLAUDE_PLUGIN_ROOT}/target/release/grug`.

Install the binary to `~/.grug-brain/bin/` (a stable location that survives plugin updates):

```bash
mkdir -p ~/.grug-brain/bin
cp "${CLAUDE_PLUGIN_ROOT}/target/release/grug" ~/.grug-brain/bin/grug
chmod +x ~/.grug-brain/bin/grug
```

Add to PATH if not already there. Check the user's shell:

```bash
echo $SHELL
```

Then check if `~/.grug-brain/bin` is already on PATH:

```bash
echo $PATH | grep -q '.grug-brain/bin' && echo "ON_PATH" || echo "NOT_ON_PATH"
```

**If NOT_ON_PATH**, append to the shell config:
- zsh: `echo 'export PATH="$HOME/.grug-brain/bin:$PATH"' >> ~/.zshrc`
- bash: `echo 'export PATH="$HOME/.grug-brain/bin:$PATH"' >> ~/.bashrc`

Also export for the current session:

```bash
export PATH="$HOME/.grug-brain/bin:$PATH"
```

Verify:

```bash
~/.grug-brain/bin/grug --version
```

## 3. Migrate old installation (if any)

Check for and remove the old bun-based grug-brain service before installing the new one. This prevents the old service from respawning and fighting over the database.

```bash
# Check for old launchd service (macOS)
launchctl list 2>/dev/null | grep 'com.grug-brain.mcp' && echo "OLD_SERVICE_FOUND" || echo "NO_OLD_SERVICE"
```

**If OLD_SERVICE_FOUND:**

```bash
# Stop and unload old bun service
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist 2>/dev/null || true
rm -f ~/Library/LaunchAgents/com.grug-brain.mcp.plist

# Kill any lingering bun processes
pkill -f 'bun.*grug-brain' 2>/dev/null || true

# Remove old MCP registration if it points to bun or HTTP
claude mcp remove grug-brain 2>/dev/null || true

sleep 2
```

**On Linux**, check for the old systemd service:

```bash
systemctl --user is-active grug-brain.service 2>/dev/null && echo "OLD_SERVICE_FOUND" || echo "NO_OLD_SERVICE"
```

If found, stop and disable it before proceeding.

Existing data is preserved — `~/.grug-brain/brains.json`, `grug.db`, and all brain directories carry over unchanged.

## 4. Service installation

Install grug-brain as a persistent background service.

### Kill stale processes

Previous sessions may have left zombie processes:

```bash
pkill -f 'grug.*serve' 2>/dev/null || true
sleep 1
```

### Install service

```bash
~/.grug-brain/bin/grug serve --install-service
```

This writes the service file and loads it. The binary handles platform detection (macOS vs Linux) automatically.

### Verify

Wait 2 seconds, then check:

```bash
sleep 2
```

- macOS: `launchctl list | grep grug`
- Linux: `systemctl --user is-active grug-brain.service`

Also check the socket:

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "Socket ready" || echo "Socket not found — check logs"
```

If the socket isn't ready, check logs:
- macOS: `~/.grug-brain/launchd-stderr.log`
- Linux: `journalctl --user -u grug-brain.service`

## 5. MCP registration

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
