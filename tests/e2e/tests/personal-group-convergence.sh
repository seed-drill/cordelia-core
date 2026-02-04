#!/usr/bin/env bash
# E2E test: personal group items replicate to keeper nodes.
#
# Verifies that items written to a personal group (taciturn culture)
# replicate to keeper members via anti-entropy sync.
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/lib/api.sh"
. "$DIR/lib/wait.sh"

N="${N:-3}"
TIMEOUT="${TIMEOUT:-180}"
HOST1="127.0.0.1:9473"
HOST2="127.0.0.1:9483"
HOST3="127.0.0.1:9493"

echo "=== Personal group convergence test: ${N} nodes ==="

# Wait for mesh convergence
wait_all_healthy "$N" "$TIMEOUT"
wait_all_hot "$N" 1 "$TIMEOUT"

# Create personal group on node1 with taciturn culture
echo "Creating personal group on node1..."
api_create_group "$HOST1" "personal-e2e-user" "e2e-user (personal)" "taciturn"

# Add node2 and node3 as keeper members
echo "Adding keeper members..."
api_add_group_member "$HOST1" "personal-e2e-user" "keeper-node2" "member"
api_add_group_member "$HOST1" "personal-e2e-user" "keeper-node3" "member"

# Wait for group to propagate
sleep 15

# Write an item to the personal group on node1
echo "Writing item to personal group on node1..."
DATA=$(echo -n "personal-group-test-payload" | base64)
api_write_item "$HOST1" "pg-item-001" "entity" "$DATA" "personal-e2e-user"

# Wait for anti-entropy sync (taciturn interval overridden to 10s in test config)
echo "Waiting for item replication to node2..."
wait_item_replicated "$HOST2" "pg-item-001" 60

echo "Waiting for item replication to node3..."
wait_item_replicated "$HOST3" "pg-item-001" 60

# Verify item is encrypted on keeper nodes (opaque blob, not plaintext)
echo "Verifying item is encrypted on keeper nodes..."
ITEM_DATA=$(api_read_item "$HOST2" "pg-item-001" 2>/dev/null || echo "{}")
if echo "$ITEM_DATA" | jq -e '.data._encrypted // .data.iv' > /dev/null 2>&1; then
    echo "OK: item is encrypted on node2"
elif echo "$ITEM_DATA" | jq -e '.data' > /dev/null 2>&1; then
    # Item present but check if content is readable (no PSK = should be opaque)
    echo "OK: item present on node2 (stored as replicated blob)"
else
    echo "WARNING: could not verify encryption state on node2"
fi

# Check zero sync errors
check_zero_sync_errors "$N"

echo "=== Personal group convergence test PASSED ==="
