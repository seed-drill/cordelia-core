#!/usr/bin/env bash
# E2e test: write item on node1, verify replication to node2.
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/lib/api.sh"
. "$DIR/lib/wait.sh"

N="${N:-3}"
TIMEOUT="${TIMEOUT:-180}"
HOST1="127.0.0.1:9473"
HOST2="127.0.0.1:9483"

echo "=== Replication test: ${N} nodes ==="

# Wait for mesh convergence
wait_all_healthy "$N" "$TIMEOUT"
wait_all_hot "$N" 1 "$TIMEOUT"

# Create a chatty group on node1
echo "Creating chatty group on node1..."
api_create_group "$HOST1" "e2e-repl-group" "E2E Replication Test" "chatty"

# Wait for group to propagate (group exchange happens on governor tick)
sleep 15

# Write an item on node1
echo "Writing test item on node1..."
api_write_item "$HOST1" "e2e-item-001" "entity" '{"test":"e2e-replication","source":"node1"}' "e2e-repl-group"

# Verify item appears on node2
echo "Waiting for item on node2..."
wait_item_replicated "$HOST2" "e2e-item-001" 60

# Verify zero sync errors
check_zero_sync_errors "$N"

echo "=== Replication test PASSED ==="
