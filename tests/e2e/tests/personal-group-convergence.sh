#!/usr/bin/env bash
# Personal group convergence test for cordelia zoned topology.
# Designed to run INSIDE the orchestrator container (direct hostname access).
#
# Test plan:
#   1. Create personal group on agent-alpha-1, edge nodes, and keeper nodes
#      (group must exist on relay/edge nodes for group_intersection routing)
#   2. Write item to personal group on agent-alpha-1
#   3. Verify item replicates to both keepers via edge relay nodes
#   4. Verify item does NOT leak to bravo or backbone nodes
#   5. Write a second item, verify it also propagates
#
# Replication path: agent -> edge (relay) -> keeper
# Uses chatty culture for eager push (immediate replication on write).
# Group must exist on edge nodes so group_intersection is computed during
# group exchange, enabling the relay push/re-push path.

set -euo pipefail

BEARER="${BEARER_TOKEN:-test-token-fixed}"
PASSED=0
FAILED=0

# --- Helpers ----------------------------------------------------------------

api() {
    local host="$1" endpoint="$2"
    local data; if [ $# -ge 3 ]; then data="$3"; else data='{}'; fi
    curl -sf --max-time 5 \
        -X POST \
        -H "Authorization: Bearer ${BEARER}" \
        -H "Content-Type: application/json" \
        -d "$data" \
        "http://${host}:9473/api/v1/${endpoint}" 2>/dev/null
}

pass() {
    echo "  PASS: $1"
    PASSED=$((PASSED + 1))
    [ "${REPORT:-0}" = "1" ] && report_test "$1" "PASS"
}

fail() {
    echo "  FAIL: $1"
    FAILED=$((FAILED + 1))
    [ "${REPORT:-0}" = "1" ] && report_test "$1" "FAIL"
}

wait_for_item() {
    local host="$1" item_id="$2" timeout="${3:-90}"
    local deadline=$((SECONDS + timeout))
    while [ $SECONDS -lt $deadline ]; do
        local result
        result=$(api "$host" "l2/read" "{\"item_id\":\"$item_id\"}" || echo "{}")
        if echo "$result" | grep -q '"data"'; then
            [ "${REPORT:-0}" = "1" ] && report_item_detected "$item_id" "$host"
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

# --- Report init (opt-in) ---------------------------------------------------

DIR="$(cd "$(dirname "$0")/.." && pwd)"

if [ "${REPORT:-0}" = "1" ]; then
    . "$DIR/lib/api.sh"
    . "$DIR/lib/report.sh"

    # Count nodes from topology
    _N=0
    _bb="${BACKBONE_COUNT:-3}"
    _bbp="${BACKBONE_PERSONAL:-0}"
    _N=$((_N + _bb + _bbp))
    IFS=',' read -ra _ods <<< "${ORG_SPEC:-alpha:2:2:2,bravo:2:2:1,charlie:1:1:0}"
    for _d in "${_ods[@]}"; do
        IFS=':' read -r _ _e _k _p <<< "$_d"
        _N=$((_N + ${_e:-2} + ${_k:-2} + ${_p:-0}))
    done

    report_init "personal-group-convergence" "$_N"
    report_snapshot "pre"
    trap 'report_finalize $?' EXIT
fi

# --- Pre-flight -------------------------------------------------------------

echo "=== Personal Group Convergence Test ==="
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

echo "[0] Pre-flight: checking required nodes..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "preflight"
for node in agent-alpha-1 keeper-alpha-1 keeper-alpha-2 edge-alpha-1; do
    if api "$node" "status" > /dev/null 2>&1; then
        pass "$node reachable"
    else
        fail "$node unreachable"
    fi
done
[ "${REPORT:-0}" = "1" ] && report_phase_end "preflight" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# Bail if pre-flight failed
if [ "$FAILED" -gt 0 ]; then
    echo "Pre-flight failed -- cannot run personal group tests"
    exit "$FAILED"
fi

# --- Test 1: Create personal group with taciturn culture --------------------

TS=$(date +%s)
PG_ID="personal-agent-alpha-1-${TS}"
PG_ITEM="pg-item-${TS}"

echo "[1] Creating personal group '${PG_ID}' on agent, edges, and keepers..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "create_personal_group"

# Create group on agent (owner), edge relays, and keepers.
# Edge nodes MUST have the group for group_intersection routing -- without it,
# the relay can't compute group_intersection and won't forward items.
for node in agent-alpha-1 edge-alpha-1 edge-alpha-2 keeper-alpha-1 keeper-alpha-2; do
    api "$node" "groups/create" \
        "{\"group_id\":\"${PG_ID}\",\"name\":\"agent-alpha-1 (personal)\",\"culture\":\"chatty\",\"security_policy\":\"standard\"}" > /dev/null 2>&1 || true
done

# Verify group exists on key nodes
for node in agent-alpha-1 edge-alpha-1 keeper-alpha-1 keeper-alpha-2; do
    GRPS=$(api "$node" "groups/list" "{}" 2>/dev/null || echo "[]")
    if echo "$GRPS" | grep -q "$PG_ID"; then
        pass "personal group created on ${node}"
    else
        fail "personal group NOT found on ${node}"
    fi
done

# Wait for group exchange so group_intersection is computed on all peers.
# GROUP_EXCHANGE_TICKS=6, governor tick=10s, so group exchange fires every 60s.
# Initial group exchange also fires on peer connect, but peers are already
# connected at this point, so we wait for the periodic exchange.
echo "  Waiting 65s for group exchange tick..."
sleep 65
[ "${REPORT:-0}" = "1" ] && report_phase_end "create_personal_group" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# --- Test 2: Write item to personal group ----------------------------------

echo "[2] Writing item to personal group on agent-alpha-1..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "write_item"

api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PG_ITEM}\",\"type\":\"entity\",\"data\":{\"test\":\"personal-group-convergence\",\"source\":\"agent-alpha-1\",\"ts\":\"${TS}\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${PG_ID}\",\"owner_id\":\"agent-alpha-1\",\"author_id\":\"agent-alpha-1\",\"key_version\":1}}" > /dev/null

[ "${REPORT:-0}" = "1" ] && report_item_written "$PG_ITEM" "agent-alpha-1"

# Verify it exists locally
if wait_for_item "agent-alpha-1" "$PG_ITEM" 5; then
    pass "item readable on agent-alpha-1 (local)"
else
    fail "item NOT readable on agent-alpha-1 after write"
fi
[ "${REPORT:-0}" = "1" ] && report_phase_end "write_item" "PASS"
echo ""

# --- Test 3: Verify replication to keepers ----------------------------------

echo "[3] Waiting for replication to keepers (chatty push, agent -> edge -> keeper)..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "replicate_to_keepers"
[ "${REPORT:-0}" = "1" ] && report_start_sampler 10

if wait_for_item "keeper-alpha-1" "$PG_ITEM" 180; then
    pass "item replicated to keeper-alpha-1"
else
    fail "item did NOT replicate to keeper-alpha-1 within timeout"
fi

if wait_for_item "keeper-alpha-2" "$PG_ITEM" 180; then
    pass "item replicated to keeper-alpha-2"
else
    fail "item did NOT replicate to keeper-alpha-2 within timeout"
fi
[ "${REPORT:-0}" = "1" ] && report_phase_end "replicate_to_keepers" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# --- Test 4: Verify isolation (no leakage to other orgs) -------------------

echo "[4] Verifying org isolation (15s window for potential leakage)..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "verify_isolation"
sleep 15

# Personal group items must NOT reach other org's keepers or agents.
# Backbone/boot nodes may relay opaque blobs (transparent posture) -- that's
# expected infrastructure behaviour, not a leak.
for node in keeper-bravo-1 agent-bravo-1 edge-bravo-1; do
    if assert_no_item "$node" "$PG_ITEM"; then
        pass "personal group item correctly absent from ${node}"
    else
        fail "personal group item LEAKED to ${node}"
    fi
done
[ "${REPORT:-0}" = "1" ] && report_phase_end "verify_isolation" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# --- Test 5: Second write (verify ongoing replication) ---------------------

PG_ITEM2="pg-item2-${TS}"
echo "[5] Second write to verify ongoing replication..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "second_write"

api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PG_ITEM2}\",\"type\":\"learning\",\"data\":{\"test\":\"personal-group-convergence-2\",\"source\":\"agent-alpha-1\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${PG_ID}\",\"owner_id\":\"agent-alpha-1\",\"author_id\":\"agent-alpha-1\",\"key_version\":1}}" > /dev/null

[ "${REPORT:-0}" = "1" ] && report_item_written "$PG_ITEM2" "agent-alpha-1"

if wait_for_item "keeper-alpha-1" "$PG_ITEM2" 180; then
    pass "second item replicated to keeper-alpha-1"
else
    fail "second item did NOT replicate to keeper-alpha-1"
fi
[ "${REPORT:-0}" = "1" ] && report_phase_end "second_write" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# --- Test 6: Taciturn culture WITHOUT edge provisioning --------------------
# This tests that dynamic relays learn the group via group exchange and
# can route items via anti-entropy sync (relay_learned_groups in
# group_intersection). No explicit group creation on edge nodes.

PG_ID2="personal-taciturn-${TS}"
PG_ITEM3="pg-taciturn-${TS}"

echo "[6] Taciturn replication without edge provisioning..."
[ "${REPORT:-0}" = "1" ] && report_phase_start "taciturn_replication"
echo "  Creating group on agent + keepers only (edges learn via group exchange)..."

for node in agent-alpha-1 keeper-alpha-1 keeper-alpha-2; do
    api "$node" "groups/create" \
        "{\"group_id\":\"${PG_ID2}\",\"name\":\"taciturn-test\",\"culture\":\"taciturn\",\"security_policy\":\"standard\"}" > /dev/null 2>&1 || true
done

# Wait for group exchange so edges learn the group from agent
echo "  Waiting 65s for group exchange tick..."
sleep 65

# Write item
api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PG_ITEM3}\",\"type\":\"entity\",\"data\":{\"test\":\"taciturn-no-edge-provision\"},\"meta\":{\"visibility\":\"group\",\"group_id\":\"${PG_ID2}\",\"owner_id\":\"agent-alpha-1\",\"author_id\":\"agent-alpha-1\",\"key_version\":1}}" > /dev/null

[ "${REPORT:-0}" = "1" ] && report_item_written "$PG_ITEM3" "agent-alpha-1"

if wait_for_item "agent-alpha-1" "$PG_ITEM3" 5; then
    pass "taciturn item readable on agent-alpha-1 (local)"
else
    fail "taciturn item NOT readable on agent-alpha-1 after write"
fi

# Taciturn: no eager push. Replication via anti-entropy sync only.
# Path: edge pulls from agent (sync), then keeper pulls from edge (sync).
# sync_base_tick = 60s, so two hops = up to 180s worst case.
# At scale (300+ nodes), sync cycles have more groups to iterate,
# so allow extra time: 120s (2x exchange) + 180s (3x sync cycles) = 300s.
echo "  Waiting for anti-entropy sync (taciturn, up to 360s)..."
if wait_for_item "keeper-alpha-1" "$PG_ITEM3" 360; then
    pass "taciturn item replicated to keeper-alpha-1 (no edge provisioning)"
else
    fail "taciturn item did NOT replicate to keeper-alpha-1"
fi
[ "${REPORT:-0}" = "1" ] && report_phase_end "taciturn_replication" "$([ "$FAILED" -eq 0 ] && echo PASS || echo FAIL)"
echo ""

# --- Results ----------------------------------------------------------------

[ "${REPORT:-0}" = "1" ] && report_stop_sampler

echo "==========================================="
echo "  RESULTS: ${PASSED} passed, ${FAILED} failed"
echo "==========================================="

exit "$FAILED"
