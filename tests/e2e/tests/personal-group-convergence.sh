#!/usr/bin/env bash
# Personal group convergence test for cordelia zoned topology.
# Designed to run INSIDE the orchestrator container (direct hostname access).
#
# Test plan:
#   1. Create personal group "personal-agent-alpha-1" on agent-alpha-1 (taciturn culture)
#   2. Add keeper-alpha-1 and keeper-alpha-2 as keeper members
#   3. Write item to personal group on agent-alpha-1
#   4. Verify item replicates to both keepers via anti-entropy sync
#   5. Verify item does NOT leak to bravo or backbone nodes
#   6. Write a second item, verify it also propagates
#
# Taciturn sync interval is 30s in test config, so budget ~90s for propagation.

set -euo pipefail

BEARER="${BEARER_TOKEN:-test-token-fixed}"
PASSED=0
FAILED=0

# --- Helpers ----------------------------------------------------------------

api() {
    local host="$1" endpoint="$2" data="${3:-{}}"
    curl -sf --max-time 5 \
        -X POST \
        -H "Authorization: Bearer ${BEARER}" \
        -H "Content-Type: application/json" \
        -d "$data" \
        "http://${host}:9473/api/v1/${endpoint}" 2>/dev/null
}

pass() { echo "  PASS: $1"; PASSED=$((PASSED + 1)); }
fail() { echo "  FAIL: $1"; FAILED=$((FAILED + 1)); }

wait_for_item() {
    local host="$1" item_id="$2" timeout="${3:-90}"
    local deadline=$((SECONDS + timeout))
    while [ $SECONDS -lt $deadline ]; do
        local result
        result=$(api "$host" "l2/read" "{\"item_id\":\"$item_id\"}" || echo "{}")
        if echo "$result" | grep -q '"data"'; then
            return 0
        fi
        sleep 3
    done
    return 1
}

assert_no_item() {
    local host="$1" item_id="$2"
    local result
    result=$(api "$host" "l2/read" "{\"item_id\":\"$item_id\"}" || echo "{}")
    if echo "$result" | grep -q '"data"'; then
        return 1
    fi
    return 0
}

# --- Pre-flight -------------------------------------------------------------

echo "=== Personal Group Convergence Test ==="
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

echo "[0] Pre-flight: checking required nodes..."
for node in agent-alpha-1 keeper-alpha-1 keeper-alpha-2 edge-alpha-1; do
    if api "$node" "status" > /dev/null 2>&1; then
        pass "$node reachable"
    else
        fail "$node unreachable"
    fi
done
echo ""

# Bail if pre-flight failed
if [ "$FAILED" -gt 0 ]; then
    echo "Pre-flight failed -- cannot run personal group tests"
    exit "$FAILED"
fi

# --- Test 1: Create personal group with taciturn culture --------------------

TS=$(date +%s)
PG_ID="personal-agent-alpha-1"
PG_ITEM="pg-item-${TS}"

echo "[1] Creating personal group '${PG_ID}' on agent-alpha-1..."
api "agent-alpha-1" "groups/create" \
    "{\"group_id\":\"${PG_ID}\",\"name\":\"agent-alpha-1 (personal)\",\"culture\":\"taciturn\",\"security_policy\":\"standard\"}" > /dev/null 2>&1 || true

# Verify group exists
GROUPS=$(api "agent-alpha-1" "groups/list" "{}" 2>/dev/null || echo "[]")
if echo "$GROUPS" | grep -q "$PG_ID"; then
    pass "personal group created on agent-alpha-1"
else
    fail "personal group NOT found on agent-alpha-1"
fi
echo ""

# --- Test 2: Add keeper members --------------------------------------------

echo "[2] Adding keepers as members..."
api "agent-alpha-1" "groups/add_member" \
    "{\"group_id\":\"${PG_ID}\",\"entity_id\":\"keeper-alpha-1\",\"role\":\"member\"}" > /dev/null 2>&1

api "agent-alpha-1" "groups/add_member" \
    "{\"group_id\":\"${PG_ID}\",\"entity_id\":\"keeper-alpha-2\",\"role\":\"member\"}" > /dev/null 2>&1

# Also add the agent itself as owner
api "agent-alpha-1" "groups/add_member" \
    "{\"group_id\":\"${PG_ID}\",\"entity_id\":\"agent-alpha-1\",\"role\":\"owner\"}" > /dev/null 2>&1

# Wait for group membership to propagate via governor tick
sleep 15

# Verify group exists on keepers (group exchange should propagate it)
for keeper in keeper-alpha-1 keeper-alpha-2; do
    KGROUPS=$(api "$keeper" "groups/list" "{}" 2>/dev/null || echo "[]")
    if echo "$KGROUPS" | grep -q "$PG_ID"; then
        pass "personal group visible on ${keeper}"
    else
        # Group might not have propagated yet via exchange -- not fatal
        echo "  INFO: personal group not yet visible on ${keeper} (may propagate with item)"
    fi
done
echo ""

# --- Test 3: Write item to personal group ----------------------------------

echo "[3] Writing item to personal group on agent-alpha-1..."
api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PG_ITEM}\",\"type\":\"entity\",\"data\":{\"test\":\"personal-group-convergence\",\"source\":\"agent-alpha-1\",\"ts\":\"${TS}\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${PG_ID}\",\"owner_id\":\"agent-alpha-1\",\"author_id\":\"agent-alpha-1\",\"key_version\":1}}" > /dev/null

# Verify it exists locally
if wait_for_item "agent-alpha-1" "$PG_ITEM" 5; then
    pass "item readable on agent-alpha-1 (local)"
else
    fail "item NOT readable on agent-alpha-1 after write"
fi
echo ""

# --- Test 4: Verify replication to keepers ----------------------------------

echo "[4] Waiting for replication to keepers (taciturn sync, ~30-90s)..."

if wait_for_item "keeper-alpha-1" "$PG_ITEM" 120; then
    pass "item replicated to keeper-alpha-1"
else
    fail "item did NOT replicate to keeper-alpha-1 within timeout"
fi

if wait_for_item "keeper-alpha-2" "$PG_ITEM" 120; then
    pass "item replicated to keeper-alpha-2"
else
    fail "item did NOT replicate to keeper-alpha-2 within timeout"
fi
echo ""

# --- Test 5: Verify isolation (no leakage to other orgs) -------------------

echo "[5] Verifying isolation (15s window for potential leakage)..."
sleep 15

for node in keeper-bravo-1 boot1 edge-bravo-1; do
    if assert_no_item "$node" "$PG_ITEM"; then
        pass "personal group item correctly absent from ${node}"
    else
        fail "personal group item LEAKED to ${node}"
    fi
done
echo ""

# --- Test 6: Second write (verify ongoing replication) ---------------------

PG_ITEM2="pg-item2-${TS}"
echo "[6] Second write to verify ongoing replication..."

api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PG_ITEM2}\",\"type\":\"learning\",\"data\":{\"test\":\"personal-group-convergence-2\",\"source\":\"agent-alpha-1\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${PG_ID}\",\"owner_id\":\"agent-alpha-1\",\"author_id\":\"agent-alpha-1\",\"key_version\":1}}" > /dev/null

if wait_for_item "keeper-alpha-1" "$PG_ITEM2" 120; then
    pass "second item replicated to keeper-alpha-1"
else
    fail "second item did NOT replicate to keeper-alpha-1"
fi
echo ""

# --- Results ----------------------------------------------------------------

echo "==========================================="
echo "  RESULTS: ${PASSED} passed, ${FAILED} failed"
echo "==========================================="

exit "$FAILED"
