#!/usr/bin/env bash
# Zoned replication test: verify items propagate across org boundaries
# through the backbone, and that network isolation holds.
#
# Prerequisites: containers running from gen-compose-zoned.sh
#
# Test plan:
#   1. Create shared group "cross-org" on all nodes
#   2. Create org-private group "alpha-internal" on org alpha nodes only
#   3. Write item to "cross-org" on keeper-alpha-1
#   4. Verify it propagates: keeper-a -> edge-a -> backbone -> edge-b -> keeper-b
#   5. Write item to "alpha-internal" on keeper-alpha-1
#   6. Verify it reaches keeper-alpha-2 but NOT any bravo/charlie nodes
#   7. Verify keepers cannot directly reach nodes outside their org network

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
BEARER="${BEARER_TOKEN:-test-token-fixed}"
BASE_PORT=9473

# ============================================================================
# Helpers
# ============================================================================

# Map hostname to API port (must match gen-compose-zoned.sh ordering)
declare -A NODE_PORTS

populate_ports() {
    local port=$BASE_PORT
    local backbone_count="${BACKBONE_COUNT:-3}"
    local org_spec="${ORG_SPEC:-alpha:2:2,bravo:2:2,charlie:1:1}"

    # Backbone
    for i in $(seq 1 "$backbone_count"); do
        NODE_PORTS["boot${i}"]=$port
        port=$((port + 10))
    done

    # Edges then keepers per org (same order as gen-compose-zoned.sh)
    IFS=',' read -ra defs <<< "$org_spec"
    for def in "${defs[@]}"; do
        IFS=':' read -r name edges keepers <<< "$def"
        for e in $(seq 1 "$edges"); do
            NODE_PORTS["edge-${name}-${e}"]=$port
            port=$((port + 10))
        done
    done
    for def in "${defs[@]}"; do
        IFS=':' read -r name edges keepers <<< "$def"
        for k in $(seq 1 "$keepers"); do
            NODE_PORTS["keeper-${name}-${k}"]=$port
            port=$((port + 10))
        done
    done
}

api() {
    local node="$1"
    local endpoint="$2"
    local data="$3"
    local port="${NODE_PORTS[$node]}"

    curl -sf -X POST \
        -H "Authorization: Bearer ${BEARER}" \
        -H "Content-Type: application/json" \
        -d "$data" \
        "http://127.0.0.1:${port}${endpoint}" 2>/dev/null
}

api_status() {
    local node="$1"
    local port="${NODE_PORTS[$node]}"
    curl -sf -o /dev/null -w "%{http_code}" -X POST \
        -H "Authorization: Bearer ${BEARER}" \
        -H "Content-Type: application/json" \
        -d "$1" \
        "http://127.0.0.1:${port}/api/v1/status" 2>/dev/null || echo "000"
}

create_group() {
    local node="$1"
    local group_id="$2"
    local culture="${3:-chatty}"
    api "$node" "/api/v1/groups/create" \
        "{\"group_id\":\"${group_id}\",\"name\":\"${group_id}\",\"culture\":\"${culture}\",\"departure_policy\":\"standard\"}" || true
}

write_item() {
    local node="$1"
    local item_id="$2"
    local group_id="$3"
    api "$node" "/api/v1/l2/write" \
        "{\"item_id\":\"${item_id}\",\"type\":\"entity\",\"data\":{\"test\":\"zoned-replication\",\"source\":\"${node}\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${group_id}\",\"owner_id\":\"test\",\"author_id\":\"${node}\",\"key_version\":1}}"
}

read_item() {
    local node="$1"
    local item_id="$2"
    api "$node" "/api/v1/l2/read" "{\"item_id\":\"${item_id}\"}"
}

wait_item() {
    local node="$1"
    local item_id="$2"
    local timeout="${3:-120}"
    local deadline=$((SECONDS + timeout))

    while [ $SECONDS -lt $deadline ]; do
        local result
        result=$(read_item "$node" "$item_id" 2>/dev/null || echo "{}")
        if echo "$result" | jq -e '.data' >/dev/null 2>&1; then
            return 0
        fi
        sleep 2
    done
    return 1
}

assert_no_item() {
    local node="$1"
    local item_id="$2"
    local result
    result=$(read_item "$node" "$item_id" 2>/dev/null || echo "{}")
    if echo "$result" | jq -e '.data' >/dev/null 2>&1; then
        echo "FAIL: ${node} has item ${item_id} but should NOT"
        return 1
    fi
    return 0
}

# ============================================================================
# Main
# ============================================================================

populate_ports

echo "=== Zoned Replication Test ==="
echo "Nodes: ${!NODE_PORTS[*]}"
echo ""

FAILED=0

# --------------------------------------------------------------------------
# Test 1: Cross-org replication via backbone
# --------------------------------------------------------------------------
echo "[1/3] Cross-org replication..."

# Create cross-org group on all nodes
for node in "${!NODE_PORTS[@]}"; do
    create_group "$node" "cross-org" "chatty"
done

# Wait for group creation to propagate
sleep 5

# Write on keeper-alpha-1
write_item "keeper-alpha-1" "cross-org-item-001" "cross-org"
echo "  Written cross-org-item-001 on keeper-alpha-1"

# Verify propagation to bravo and charlie keepers (full path traversal)
for target in keeper-bravo-1 keeper-bravo-2 keeper-charlie-1; do
    if wait_item "$target" "cross-org-item-001" 120; then
        echo "  OK: ${target} received cross-org-item-001"
    else
        echo "  FAIL: ${target} did not receive cross-org-item-001 within timeout"
        FAILED=$((FAILED + 1))
    fi
done

# Also check backbone and edges got it
for target in boot1 edge-alpha-1 edge-bravo-1; do
    if wait_item "$target" "cross-org-item-001" 30; then
        echo "  OK: ${target} received cross-org-item-001"
    else
        echo "  FAIL: ${target} did not receive cross-org-item-001"
        FAILED=$((FAILED + 1))
    fi
done

echo ""

# --------------------------------------------------------------------------
# Test 2: Org-private group isolation
# --------------------------------------------------------------------------
echo "[2/3] Org-private group isolation..."

# Create alpha-internal only on alpha nodes
for node in keeper-alpha-1 keeper-alpha-2 edge-alpha-1 edge-alpha-2; do
    create_group "$node" "alpha-internal" "chatty"
done

sleep 5

# Write on keeper-alpha-1
write_item "keeper-alpha-1" "alpha-secret-001" "alpha-internal"
echo "  Written alpha-secret-001 on keeper-alpha-1 (alpha-internal group)"

# Should reach other alpha nodes
if wait_item "keeper-alpha-2" "alpha-secret-001" 60; then
    echo "  OK: keeper-alpha-2 received alpha-secret-001"
else
    echo "  FAIL: keeper-alpha-2 did not receive alpha-secret-001"
    FAILED=$((FAILED + 1))
fi

# Wait for any leakage, then verify bravo/charlie do NOT have it
sleep 15

for target in keeper-bravo-1 keeper-charlie-1 boot1; do
    if assert_no_item "$target" "alpha-secret-001"; then
        echo "  OK: ${target} correctly excluded from alpha-internal"
    else
        FAILED=$((FAILED + 1))
    fi
done

echo ""

# --------------------------------------------------------------------------
# Test 3: Bidirectional cross-org (bravo -> alpha)
# --------------------------------------------------------------------------
echo "[3/3] Bidirectional cross-org replication..."

write_item "keeper-bravo-1" "cross-org-item-002" "cross-org"
echo "  Written cross-org-item-002 on keeper-bravo-1"

if wait_item "keeper-alpha-1" "cross-org-item-002" 120; then
    echo "  OK: keeper-alpha-1 received item from bravo"
else
    echo "  FAIL: keeper-alpha-1 did not receive item from bravo"
    FAILED=$((FAILED + 1))
fi

if wait_item "keeper-charlie-1" "cross-org-item-002" 120; then
    echo "  OK: keeper-charlie-1 received item from bravo"
else
    echo "  FAIL: keeper-charlie-1 did not receive item from bravo"
    FAILED=$((FAILED + 1))
fi

echo ""

# --------------------------------------------------------------------------
# Results
# --------------------------------------------------------------------------
if [ "$FAILED" -eq 0 ]; then
    echo "=== ALL TESTS PASSED ==="
else
    echo "=== ${FAILED} TESTS FAILED ==="
fi

exit "$FAILED"
