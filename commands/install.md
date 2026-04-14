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

Try `gh` first, then fall back to `curl`:

```bash
mkdir -p ~/.grug-brain/bin
REPO="ryanthedev/grug-brain.mcp"
TARBALL="/tmp/grug-${TARGET}.tar.gz"
DOWNLOADED=false

if command -v gh >/dev/null 2>&1; then
  LATEST=$(gh release list --repo "$REPO" --limit 1 --exclude-drafts --exclude-pre-releases --json tagName -q '.[0].tagName' 2>/dev/null)
  if [[ -n "$LATEST" ]]; then
    gh release download "$LATEST" --repo "$REPO" --pattern "grug-${TARGET}.tar.gz" --dir /tmp --clobber 2>/dev/null && DOWNLOADED=true
  fi
fi

if [[ "$DOWNLOADED" != "true" ]] && command -v curl >/dev/null 2>&1; then
  LATEST=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
  if [[ -n "$LATEST" ]]; then
    curl -fsSL "https://github.com/$REPO/releases/download/$LATEST/grug-${TARGET}.tar.gz" -o "$TARBALL" 2>/dev/null && DOWNLOADED=true
  fi
fi

if [[ "$DOWNLOADED" == "true" ]]; then
  tar xzf "$TARBALL" -C ~/.grug-brain/bin && chmod +x ~/.grug-brain/bin/grug && rm -f "$TARBALL" && echo "DOWNLOADED" || echo "EXTRACT_FAILED"
else
  echo "DOWNLOAD_FAILED"
fi
```

If DOWNLOADED, skip to **step 3**. If DOWNLOAD_FAILED or EXTRACT_FAILED, fall through to build from source.

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

Kill stale processes and install/reinstall the service:

```bash
pkill -f 'grug serve' 2>/dev/null || true
sleep 1
~/.grug-brain/bin/grug serve --install-service
sleep 2
```

Verify:

```bash
[[ -S ~/.grug-brain/grug.sock ]] && echo "Socket ready" || echo "Socket not found"
```

If socket not found, check `~/.grug-brain/launchd-stderr.log` (macOS) or `journalctl --user -u grug-brain.service` (Linux) and show the last 20 lines.

## 5. Verify MCP

The plugin registers the MCP server automatically via `plugin.json`. Verify the binary version and that the background service socket exists:

```bash
~/.grug-brain/bin/grug --version
[[ -S ~/.grug-brain/grug.sock ]] && echo "MCP ready" || echo "MCP not ready"
```

If MCP not ready, the background service isn't running — revisit step 4.

## 6. Configure brains

Skip this section if mode is UPDATE.

The server creates a default `brains.json` on first start. Check what was created:

```bash
cat ~/.grug-brain/brains.json 2>/dev/null
```

Show the current brains to the user. Ask if they want to customize:

1. Ask: "Want a shared brain that syncs across machines?" — name "hive", needs git remote URL, dir `~/.grug-brain/memories`, writable, syncInterval 60.
2. Ask: "Any other brains? Docs repos, reference material?" — get name, dir, flat or not.

Example brains.json:

```json
[
  {"name": "self", "dir": "~/.grug-brain/self", "primary": true, "writable": true},
  {"name": "hive", "dir": "~/.grug-brain/memories", "writable": true, "git": "git@github.com:user/brain.git", "syncInterval": 60}
]
```

Write updates to `~/.grug-brain/brains.json` if the user made changes.

## 7. Git setup (first install only)

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

## 8. Summary

Report:
- Mode: fresh install or update
- Install method: prebuilt binary or built from source
- grug version (`~/.grug-brain/bin/grug --version`)
- Service: running / failed (with log path)
- MCP: ready / not ready
- Brains: name, dir, file count, writable, git-synced (list on install, skip on update)
