---
description: Install or update grug-brain. Downloads prebuilt binary or builds from source.
allowed-tools: Bash, Read, Write, AskUserQuestion
---

Run these steps in order. Fix problems as you find them. Report a summary at the end.

## 1. Detect mode

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "UPDATE" || echo "INSTALL"
```

If UPDATE, tell the user you're upgrading to the latest version. If INSTALL, tell them you're setting up grug-brain for the first time.

## 2. Get the binary

Try downloading a prebuilt binary from GitHub releases first. Fall back to building from source only if no prebuilt binary is available.

### Detect platform

```bash
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$OS-$ARCH" in
  darwin-arm64) TARGET="aarch64-apple-darwin" ;;
  *) TARGET="" ;;
esac
echo "TARGET=$TARGET"
```

If TARGET is empty, skip to **Build from source** below.

### Download prebuilt binary

Get the latest release tag and download:

```bash
LATEST=$(gh release list --repo ryanthedev/grug-brain.mcp --limit 1 --exclude-drafts --exclude-pre-releases --json tagName -q '.[0].tagName' 2>/dev/null)
echo "LATEST=$LATEST"
```

If `gh` is not installed or the command fails, skip to **Build from source**.

```bash
mkdir -p ~/.grug-brain/bin
gh release download "$LATEST" --repo ryanthedev/grug-brain.mcp --pattern "grug-${TARGET}.tar.gz" --dir /tmp 2>/dev/null && \
  tar xzf "/tmp/grug-${TARGET}.tar.gz" -C ~/.grug-brain/bin && \
  chmod +x ~/.grug-brain/bin/grug && \
  echo "DOWNLOADED" || echo "DOWNLOAD_FAILED"
```

If DOWNLOADED, skip to **Check PATH**. If DOWNLOAD_FAILED, fall through to build from source.

### Build from source

```bash
cargo --version
```

If cargo is missing:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

Build and install:

```bash
cd "${CLAUDE_PLUGIN_ROOT}"
cargo build --release
mkdir -p ~/.grug-brain/bin
cp "${CLAUDE_PLUGIN_ROOT}/target/release/grug" ~/.grug-brain/bin/grug
chmod +x ~/.grug-brain/bin/grug
```

### Check PATH

```bash
echo $PATH | grep -q '.grug-brain/bin' && echo "ON_PATH" || echo "NOT_ON_PATH"
```

If NOT_ON_PATH, append to shell config (~/.zshrc or ~/.bashrc based on $SHELL) and export for the current session.

## 3. Migrate old bun service (first install only)

Skip this section if mode is UPDATE.

```bash
launchctl list 2>/dev/null | grep 'com.grug-brain.mcp' && echo "OLD_SERVICE" || echo "CLEAN"
```

If OLD_SERVICE:

```bash
launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.mcp.plist 2>/dev/null || true
rm -f ~/Library/LaunchAgents/com.grug-brain.mcp.plist
pkill -f 'bun.*grug-brain' 2>/dev/null || true
claude mcp remove grug-brain 2>/dev/null || true
sleep 2
```

## 4. Install or restart service

Stop existing service first (both modes — harmless if not running):

- macOS: `launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/com.grug-brain.server.plist 2>/dev/null || true`
- Linux: `systemctl --user stop grug-brain.service 2>/dev/null || true`

```bash
pkill -f 'grug.*serve' 2>/dev/null || true
sleep 2
~/.grug-brain/bin/grug serve --install-service
sleep 2
```

Verify:

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "Socket ready" || echo "Socket not found"
```

If socket not found, check `~/.grug-brain/launchd-stderr.log` (macOS) or `journalctl --user -u grug-brain.service` (Linux) and show the last 20 lines.

## 5. Configure brains (first install only)

Skip this section if mode is UPDATE.

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

### No config file

Create interactively:

1. **self brain** (primary, local-only): dir `~/.grug-brain/self`, primary, no git.
2. Ask: "Want a shared brain that syncs across machines?" — name "hive", needs git remote URL, dir `~/.grug-brain/memories`, writable, syncInterval 60.
3. Ask: "Any other brains? Docs repos, reference material?" — get name, dir, flat or not.

Write the JSON array to `~/.grug-brain/brains.json`.

### Config file exists

Show current brains. Ask if they want to add another.

## 6. Git setup (first install only)

Skip this section if mode is UPDATE.

For each brain with a `git` field, check if initialized:

```bash
cd "<brain-dir>" && git rev-parse --git-dir 2>/dev/null
```

If not:

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
- Mode: fresh install or update
- Install method: prebuilt binary or built from source
- grug version (`~/.grug-brain/bin/grug --version`)
- Service: running / failed (with log path)
- Brains: name, dir, file count, writable, git-synced (list on install, skip on update)
