#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "Building..."
cargo build --release 2>&1

echo "Restarting service..."
launchctl kickstart -k "gui/$(id -u)/com.grug-brain.server"

sleep 1
if [[ -S ~/.grug-brain/grug.sock ]]; then
  echo "Ready — $(~/.grug-brain/bin/grug --version)"
else
  echo "Socket not found — check ~/.grug-brain/launchd-stderr.log"
  exit 1
fi
