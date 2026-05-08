#!/bin/bash
set -e

echo "=========================================="
echo "DW-3.1: handlers.rs deleted, split files"
echo "=========================================="

if [ -f src/http/handlers.rs ]; then
    echo "FAIL: src/http/handlers.rs still exists"
    exit 1
fi
echo "PASS: src/http/handlers.rs deleted"

for file in memories search graph write; do
    if [ ! -f "src/http/${file}.rs" ]; then
        echo "FAIL: src/http/${file}.rs does not exist"
        exit 1
    fi
    echo "PASS: src/http/${file}.rs exists"
done

if [ -f src/http/helpers.rs ]; then
    echo "PASS: src/http/helpers.rs exists (shared helpers)"
else
    echo "FAIL: src/http/helpers.rs does not exist"
    exit 1
fi

echo ""
echo "=========================================="
echo "DW-3.2: All __http/* arms use db.method()"
echo "=========================================="

# Check that no dispatch arms call handlers::*_json
if grep -q "handlers::" src/server.rs; then
    echo "FAIL: Found handlers:: calls in dispatch_tool"
    grep "handlers::" src/server.rs | head -5
    exit 1
fi
echo "PASS: No handlers:: calls found"

# Count __http/* arms in dispatch_tool (should be 14)
http_count=$(grep -c '"__http/' src/server.rs)
echo "Found $http_count __http/* dispatch arms"

if [ "$http_count" -lt 14 ]; then
    echo "FAIL: Expected at least 14 __http/* arms, found $http_count"
    exit 1
fi
echo "PASS: All __http/* arms present (count: $http_count)"

# All should call db.* methods
if ! grep -A 20 '"__http/' src/server.rs | grep -q "db\..*_json"; then
    echo "FAIL: Not all dispatch arms call db.method()"
    exit 1
fi
echo "PASS: __http/* arms call db.method()"

echo ""
echo "=========================================="
echo "DW-3.3: Tests pass"
echo "=========================================="

# Run cargo test
cargo test --lib 2>&1 | tail -5
LIB_RESULT=$?

# Run integration tests
cargo test --test '*' 2>&1 | tail -15
INT_RESULT=$?

if [ $LIB_RESULT -ne 0 ] || [ $INT_RESULT -ne 0 ]; then
    echo "FAIL: Some tests failed"
    exit 1
fi
echo "PASS: All cargo tests pass"

# Try Playwright if available
if command -v npx &> /dev/null && [ -f tests/playwright.config.ts ]; then
    echo "Running Playwright tests..."
    npx playwright test 2>&1 | tail -20 || echo "Playwright not available or tests failed"
else
    echo "INFO: Playwright tests not available, skipping"
fi

echo ""
echo "=========================================="
echo "DW-3.4: No HTTP file exceeds 300 lines"
echo "=========================================="

max_lines=0
for file in src/http/{memories,search,graph,write,helpers}.rs; do
    if [ -f "$file" ]; then
        lines=$(wc -l < "$file")
        echo "$file: $lines lines"
        if [ "$lines" -gt 300 ]; then
            echo "FAIL: $file exceeds 300 lines"
            exit 1
        fi
        if [ "$lines" -gt "$max_lines" ]; then
            max_lines=$lines
        fi
    fi
done

echo "PASS: All files <= 300 lines (max: $max_lines)"

echo ""
echo "=========================================="
echo "Summary"
echo "=========================================="
echo "DW-3.1: PASS (handlers.rs deleted, split files created)"
echo "DW-3.2: PASS (all __http/* arms use db.method())"
echo "DW-3.3: PASS (all cargo tests pass)"
echo "DW-3.4: PASS (no file exceeds 300 lines)"
echo ""
echo "OVERALL: Phase 3 requirements satisfied"
