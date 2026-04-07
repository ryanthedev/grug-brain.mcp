#!/usr/bin/env bash
# Auto-update grug-brain binary on session start if plugin version differs.
# Logs to ~/.grug-brain/auto-update.log. Exits 0 always — never blocks session.
set -euo pipefail

PLUGIN_ROOT="${CLAUDE_PLUGIN_ROOT:-}"
GRUG_BIN="$HOME/.grug-brain/bin/grug"
REPO="ryanthedev/grug-brain.mcp"
LOG="$HOME/.grug-brain/auto-update.log"

log() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*" >> "$LOG"; }

# No plugin root — nothing to compare against
[ -z "$PLUGIN_ROOT" ] && exit 0

# No binary installed yet — /install handles first-time setup
[ ! -x "$GRUG_BIN" ] && exit 0

# Read plugin version from plugin.json
PLUGIN_JSON="$PLUGIN_ROOT/.claude-plugin/plugin.json"
[ ! -f "$PLUGIN_JSON" ] && exit 0

WANT=$(grep '"version"' "$PLUGIN_JSON" | head -1 | sed 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
[ -z "$WANT" ] && exit 0

# Read installed binary version
HAVE=$("$GRUG_BIN" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "")
[ -z "$HAVE" ] && exit 0

# Same version — nothing to do (fast path)
[ "$WANT" = "$HAVE" ] && exit 0

log "update needed: $HAVE → $WANT"

# Version mismatch — try to download prebuilt binary
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
case "$OS-$ARCH" in
  darwin-arm64) TARGET="aarch64-apple-darwin" ;;
  *)
    log "no prebuilt binary for $OS-$ARCH, skipping"
    exit 0
    ;;
esac

TAG="v$WANT"
TARBALL="/tmp/grug-${TARGET}.tar.gz"

# Try gh first, fall back to curl
if command -v gh >/dev/null 2>&1; then
  if ! gh release download "$TAG" --repo "$REPO" --pattern "grug-${TARGET}.tar.gz" --dir /tmp --clobber 2>>"$LOG"; then
    log "gh download failed for $TAG"
    exit 0
  fi
elif command -v curl >/dev/null 2>&1; then
  if ! curl -fsSL "https://github.com/$REPO/releases/download/$TAG/grug-${TARGET}.tar.gz" -o "$TARBALL" 2>>"$LOG"; then
    log "curl download failed for $TAG"
    exit 0
  fi
else
  log "neither gh nor curl found"
  exit 0
fi

if [ ! -f "$TARBALL" ]; then
  log "tarball not found after download"
  exit 0
fi

# Extract and replace
if ! tar xzf "$TARBALL" -C "$HOME/.grug-brain/bin" 2>>"$LOG"; then
  log "tar extraction failed"
  exit 0
fi
chmod +x "$GRUG_BIN"
rm -f "$TARBALL"

# Restart the background service
if [ "$(uname -s)" = "Darwin" ]; then
  launchctl kickstart -k "gui/$(id -u)/com.grug-brain.server" 2>>"$LOG" || log "launchctl restart failed"
else
  systemctl --user restart grug-brain.service 2>>"$LOG" || log "systemctl restart failed"
fi

log "updated: $HAVE → $WANT"
echo "grug-brain updated: $HAVE → $WANT" >&2
exit 0
