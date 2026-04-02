#!/usr/bin/env bash
set -euo pipefail

cd /Users/r/repos/grug-brain.mcp

echo "=== cargo build ==="
cargo build 2>&1
echo "EXIT: $?"

echo ""
echo "=== cargo test ==="
cargo test 2>&1
echo "EXIT: $?"
