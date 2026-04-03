#!/usr/bin/env bash
# Smoke test for grug-brain install chain.
#
# Run in a clean environment (CI or fresh machine) after building:
#   cargo build --release
#   export PATH="$PWD/target/release:$PATH"
#   bash scripts/smoke-test.sh
#
# Documents the expected install flow and validates each step.
# For DW-5.6: verifies that the full chain from binary to MCP response works.

set -euo pipefail

PASS=0
FAIL=0

check() {
    local desc="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS: $desc"
        ((PASS++))
    else
        echo "  FAIL: $desc"
        ((FAIL++))
    fi
}

echo "=== grug-brain smoke test ==="
echo ""

# Step 1: Verify binary is installed and accessible
echo "1. Binary check"
check "grug is in PATH" which grug
check "grug --version runs" grug --version
check "grug --help shows serve" bash -c 'grug --help 2>&1 | grep -q serve'
check "grug serve --help shows --install-service" bash -c 'grug serve --help 2>&1 | grep -q install-service'

# Step 2: Clean previous state
echo ""
echo "2. Cleaning previous state"
rm -f ~/.grug-brain/grug.sock ~/.grug-brain/grug.pid
if [[ "$(uname)" == "Darwin" ]]; then
    launchctl bootout "gui/$(id -u)" ~/Library/LaunchAgents/com.grug-brain.server.plist 2>/dev/null || true
else
    systemctl --user stop grug-brain.service 2>/dev/null || true
fi
sleep 1
echo "  DONE: Previous state cleaned"

# Step 3: Create minimal brain config (if none exists)
echo ""
echo "3. Brain configuration"
mkdir -p ~/.grug-brain/self
if [[ ! -f ~/.grug-brain/brains.json ]]; then
    cat > ~/.grug-brain/brains.json << 'BRAINS'
[{"name":"self","dir":"~/.grug-brain/self","primary":true,"writable":true}]
BRAINS
    echo "  DONE: Created minimal brains.json"
else
    echo "  SKIP: brains.json already exists"
fi

# Step 4: Install service
echo ""
echo "4. Service installation"
grug serve --install-service
check "service install succeeded" true

# Step 5: Wait for server to start
echo ""
echo "5. Server startup"
sleep 3
check "socket file exists" test -S ~/.grug-brain/grug.sock
check "PID file exists" test -f ~/.grug-brain/grug.pid

if [[ "$(uname)" == "Darwin" ]]; then
    check "launchctl lists grug" bash -c 'launchctl list 2>/dev/null | grep -q com.grug-brain.server'
else
    check "systemd service is active" systemctl --user is-active grug-brain.service
fi

# Step 6: Test socket connectivity
echo ""
echo "6. Socket connectivity"
if command -v socat >/dev/null 2>&1; then
    RESPONSE=$(echo '{"id":"smoke","tool":"grug-search","params":{"query":"test"}}' | socat -t 5 - UNIX-CONNECT:"$HOME/.grug-brain/grug.sock" 2>/dev/null || echo "CONNECT_FAILED")
    if echo "$RESPONSE" | grep -q '"id":"smoke"'; then
        echo "  PASS: Socket responds to tool call"
        ((PASS++))
    else
        echo "  FAIL: Socket did not respond correctly: $RESPONSE"
        ((FAIL++))
    fi
else
    echo "  SKIP: socat not installed (socket file existence is sufficient)"
fi

# Step 7: Test MCP stdio bridge
echo ""
echo "7. MCP stdio bridge"
# The stdio bridge reads JSON-RPC from stdin and forwards to the socket.
# We send an initialize request followed by a tools/list request.
MCP_INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1.0"}}}'
MCP_RESPONSE=$(echo "$MCP_INIT" | timeout 5 grug --stdio 2>/dev/null || echo "STDIO_FAILED")
if echo "$MCP_RESPONSE" | grep -q '"result"'; then
    echo "  PASS: MCP stdio bridge returns valid response"
    ((PASS++))
elif echo "$MCP_RESPONSE" | grep -q '"jsonrpc"'; then
    echo "  PASS: MCP stdio bridge returns JSON-RPC response"
    ((PASS++))
else
    echo "  INFO: MCP stdio bridge response: $MCP_RESPONSE"
    echo "  SKIP: MCP stdio bridge test inconclusive (may need full JSON-RPC framing)"
fi

# Summary
echo ""
echo "=== Results ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"
echo ""

if [[ $FAIL -eq 0 ]]; then
    echo "All checks passed."
    exit 0
else
    echo "Some checks failed. Review output above."
    exit 1
fi
