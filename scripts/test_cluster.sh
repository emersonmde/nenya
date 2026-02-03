#!/bin/bash
# Quick cluster diagnostic

echo "=== Cluster Diagnostic ==="
echo ""

echo "1. Checking if nodes are running..."
for port in 8080 8090 8100; do
    if lsof -i :$port >/dev/null 2>&1; then
        echo "  ✓ Port $port: Process running"
    else
        echo "  ✗ Port $port: No process"
    fi
done

echo ""
echo "2. Checking node health..."
for port in 8080 8090 8100; do
    result=$(curl -s -m 1 http://127.0.0.1:$port/health 2>/dev/null)
    if [ -n "$result" ]; then
        echo "  ✓ Node $port:"
        echo "     $result" | jq -c .
    else
        echo "  ✗ Node $port: No response"
    fi
done

echo ""
echo "3. Testing throttle endpoint (port 8080)..."
result=$(curl -s -m 1 -X POST http://127.0.0.1:8080/should_throttle \
    -H 'Content-Type: application/json' \
    -d '{"scope":"test"}' 2>/dev/null)
if [ -n "$result" ]; then
    echo "  ✓ Response:"
    echo "     $result" | jq .
else
    echo "  ✗ No response"
fi

echo ""
echo "4. Checking for load generator process..."
if pgrep -f "cluster_load_generator" >/dev/null; then
    echo "  ✓ Load generator is running"
else
    echo "  ✗ Load generator is NOT running"
fi

echo ""
echo "=== End Diagnostic ==="
