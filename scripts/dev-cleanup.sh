#!/usr/bin/env bash
#
# One-time cleanup: collapse duplicate launchd units down to a single
# grug serve process backed by the dev binary.
#
# Background:
#   Two launchd plists (com.grug-brain.mcp + com.grug-brain.server) were
#   both running `grug serve` with KeepAlive=true and racing for the same
#   socket / HTTP port. Whichever lost spammed "another server is running"
#   on every retry, and the launched process wasn't necessarily the one
#   built from the current source tree.
#
# What this script does:
#   1. Unload both plists and kill any stray `grug serve` processes.
#   2. Remove the redundant com.grug-brain.mcp plist.
#   3. Make ~/.grug-brain/bin/grug a real symlink to the dev binary
#      (CLAUDE.md says it should be one; verify and fix if not).
#   4. Reload com.grug-brain.server (the canonical unit dev-reload.sh
#      targets).
#   5. Wait for the HTTP server and report the URL.
#
# Idempotent: safe to re-run.

set -euo pipefail

cd "$(dirname "$0")/.."

REPO_BIN="$(pwd)/target/release/grug"
INSTALL_BIN="$HOME/.grug-brain/bin/grug"
SOCK="$HOME/.grug-brain/grug.sock"
PORT_FILE="$HOME/.grug-brain/serve.port"
MCP_PLIST="$HOME/Library/LaunchAgents/com.grug-brain.mcp.plist"
SERVER_PLIST="$HOME/Library/LaunchAgents/com.grug-brain.server.plist"

if [[ ! -f "$SERVER_PLIST" ]]; then
  echo "error: $SERVER_PLIST not found — install grug as a service first" >&2
  exit 1
fi

if [[ ! -x "$REPO_BIN" ]]; then
  echo "Building release binary..."
  cargo build --release
fi

echo "Unloading launchd units..."
launchctl unload "$MCP_PLIST" 2>/dev/null || true
launchctl unload "$SERVER_PLIST" 2>/dev/null || true

echo "Killing any stray grug serve processes..."
pkill -f "grug serve" 2>/dev/null || true

# Wait for sockets/ports to release.
for _ in 1 2 3 4 5; do
  if ! pgrep -f "grug serve" >/dev/null; then break; fi
  sleep 1
done

if pgrep -f "grug serve" >/dev/null; then
  echo "warning: grug serve still running after pkill; trying SIGKILL"
  pkill -9 -f "grug serve" 2>/dev/null || true
  sleep 1
fi

# Stale socket / port file from an unclean shutdown.
rm -f "$SOCK" "$PORT_FILE"

echo "Removing duplicate mcp plist..."
if [[ -f "$MCP_PLIST" ]]; then
  rm -f "$MCP_PLIST"
fi

echo "Linking $INSTALL_BIN -> $REPO_BIN..."
mkdir -p "$(dirname "$INSTALL_BIN")"
# Replace whatever is there (file or symlink) with a symlink to the dev binary.
rm -f "$INSTALL_BIN"
ln -s "$REPO_BIN" "$INSTALL_BIN"

echo "Loading com.grug-brain.server..."
launchctl load "$SERVER_PLIST"

echo "Waiting for HTTP server..."
PORT=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  if [[ -f "$PORT_FILE" ]]; then
    PORT="$(cat "$PORT_FILE")"
    if curl -sf -m 2 "http://127.0.0.1:${PORT}/api/healthz" >/dev/null; then
      break
    fi
  fi
done

if [[ -z "$PORT" ]]; then
  echo "error: serve.port not written within 10s — check ~/.grug-brain/launchd-stderr.log" >&2
  exit 1
fi

if ! curl -sf -m 2 "http://127.0.0.1:${PORT}/api/healthz" >/dev/null; then
  echo "error: HTTP server bound on $PORT but not responding — check launchd-stderr.log" >&2
  exit 1
fi

URL="http://127.0.0.1:${PORT}"
echo
echo "Ready — single grug serve process. UI at: $URL"

if [[ "${1:-}" != "--no-open" ]] && command -v open >/dev/null 2>&1; then
  open "$URL"
fi
