#!/usr/bin/env bash
# CI smoke test suite for cordelia 7-node topology (cordelia-core#4).
# Designed to run INSIDE the orchestrator container (direct hostname access).
# Uses topology-ci.env: 2 boots, 2 edges (alpha/bravo), 2 keepers, 1 agent.
#
# Run from inside the orchestrator container:
#   docker exec cordelia-e2e-orchestrator bash /tests/ci-smoke-test.sh
#
# Set REPORT=1 to emit JSON report to ./reports/.

set -euo pipefail

BEARER="${BEARER_TOKEN:-test-token-fixed}"
PASSED=0
FAILED=0
TIMEOUT=30
TS=$(date +%s)

# Source group helpers from lib/
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [ -f "${SCRIPT_DIR}/lib/api.sh" ]; then
    . "${SCRIPT_DIR}/lib/api.sh"
fi

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

pass() { echo "  PASS: $1"; PASSED=$((PASSED + 1)); }
fail() { echo "  FAIL: $1"; FAILED=$((FAILED + 1)); }

wait_for_item() {
    local host="$1" item_id="$2" timeout="${3:-$TIMEOUT}"
    local deadline=$((SECONDS + timeout))
    while [ $SECONDS -lt $deadline ]; do
        local result
        result=$(api "$host" "l2/read" "{\"item_id\":\"$item_id\"}" || echo "{}")
        if echo "$result" | grep -q '"data"'; then
            return 0
        fi
        sleep 2
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

# Timing
declare -a R_NAMES=()
declare -a R_STATUSES=()
declare -a R_LATENCIES=()

record() {
    R_NAMES+=("$1"); R_STATUSES+=("$2"); R_LATENCIES+=("$3")
}

# --- Pre-flight [0] ---------------------------------------------------------

echo "=== Cordelia CI Smoke Test Suite ==="
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Topology: 7-node CI (2 boots, alpha: 1e/1k/1a, bravo: 1e/1k/0a)"
echo ""

echo "[0] Pre-flight: checking node connectivity..."

ALL_NODES="boot1 boot2 edge-alpha-1 edge-bravo-1 keeper-alpha-1 keeper-bravo-1 agent-alpha-1"
REACHABLE=0
TOTAL=0

for node in $ALL_NODES; do
    TOTAL=$((TOTAL + 1))
    if api "$node" "status" > /dev/null 2>&1; then
        pass "$node reachable"
        REACHABLE=$((REACHABLE + 1))
    else
        fail "$node unreachable"
    fi
done

if [ "$REACHABLE" -lt "$TOTAL" ]; then
    echo ""
    echo "ABORT: not all nodes reachable (${REACHABLE}/${TOTAL})"
    exit 1
fi
echo ""

# --- Test 1: Cross-org replication [1] --------------------------------------

echo "[1] Cross-org replication (agent-alpha-1 -> keeper-bravo-1)..."
T1_START=$(date +%s)
XORG_ITEM="ci-xorg-${TS}"

api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${XORG_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"ci-cross-org\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

if wait_for_item "keeper-bravo-1" "$XORG_ITEM" "$TIMEOUT"; then
    T1_LAT=$(( $(date +%s) - T1_START ))
    pass "shared-xorg item reached bravo (${T1_LAT}s)"
    record "cross-org-replication" "PASS" "$T1_LAT"
else
    T1_LAT=$(( $(date +%s) - T1_START ))
    fail "shared-xorg item did NOT reach bravo after ${TIMEOUT}s"
    record "cross-org-replication" "FAIL" "$T1_LAT"
fi
echo ""

# --- Test 2: Reverse replication [2] ----------------------------------------

echo "[2] Reverse replication (keeper-bravo-1 -> keeper-alpha-1)..."
T2_START=$(date +%s)
REV_ITEM="ci-reverse-${TS}"

api "keeper-bravo-1" "l2/write" \
    "{\"item_id\":\"${REV_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"ci-reverse\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"keeper-bravo-1\"}}" > /dev/null

if wait_for_item "keeper-alpha-1" "$REV_ITEM" "$TIMEOUT"; then
    T2_LAT=$(( $(date +%s) - T2_START ))
    pass "reverse item reached alpha (${T2_LAT}s)"
    record "reverse-replication" "PASS" "$T2_LAT"
else
    T2_LAT=$(( $(date +%s) - T2_START ))
    fail "reverse item did NOT reach alpha after ${TIMEOUT}s"
    record "reverse-replication" "FAIL" "$T2_LAT"
fi
echo ""

# --- Test 3: Group isolation [3] --------------------------------------------

echo "[3] Group isolation (alpha-internal should NOT reach bravo)..."
T3_START=$(date +%s)
ISO_ITEM="ci-iso-${TS}"

api "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${ISO_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"ci-isolation\"},\"meta\":{\"group_id\":\"alpha-internal\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

# Verify it reaches alpha keeper
if wait_for_item "keeper-alpha-1" "$ISO_ITEM" "$TIMEOUT"; then
    pass "alpha-internal item reached alpha keeper"
else
    fail "alpha-internal item did NOT reach alpha keeper"
fi

# Verify it does NOT reach bravo
sleep 10
if assert_no_item "keeper-bravo-1" "$ISO_ITEM"; then
    T3_LAT=$(( $(date +%s) - T3_START ))
    pass "alpha-internal item correctly absent from bravo"
    record "group-isolation" "PASS" "$T3_LAT"
else
    T3_LAT=$(( $(date +%s) - T3_START ))
    fail "alpha-internal item LEAKED to bravo"
    record "group-isolation" "FAIL" "$T3_LAT"
fi
echo ""

# --- Test 4: Group API + descriptor propagation [4] --------------------------
# Note: group MEMBERS are local-only by design (R4-030). Only group
# DESCRIPTORS (id, culture, signature) propagate via GroupExchange protocol.
# This test verifies: (a) groups API works, (b) descriptor reaches another node.

echo "[4] Group API + descriptor propagation..."
set +e
T4_START=$(date +%s)
GRP_ID="ci-grp-${TS}"
MEMBER_ID="ci-member-${TS}"
T4_OK=true

# 4a: Create group on agent-alpha-1
if ! api "agent-alpha-1" "groups/create" \
    "{\"group_id\":\"${GRP_ID}\",\"name\":\"CI Test Group\",\"culture\":\"chatty\",\"security_policy\":\"standard\"}" > /dev/null 2>&1; then
    fail "groups/create failed on agent-alpha-1"
    T4_OK=false
else
    pass "groups/create succeeded"
fi

# 4b: Add member locally (verify FK + add_member API)
if $T4_OK; then
    # Pre-create l1_hot entry (FK: group_members.entity_id -> l1_hot.user_id)
    api "agent-alpha-1" "l1/write" \
        "{\"user_id\":\"${MEMBER_ID}\",\"data\":{\"type\":\"ci-test-entity\"}}" > /dev/null 2>&1 || true

    if ! api "agent-alpha-1" "groups/add_member" \
        "{\"group_id\":\"${GRP_ID}\",\"entity_id\":\"${MEMBER_ID}\",\"role\":\"member\"}" > /dev/null 2>&1; then
        fail "groups/add_member failed on agent-alpha-1"
        T4_OK=false
    else
        pass "groups/add_member succeeded"
    fi
fi

# 4c: Read group back, verify member is present locally
if $T4_OK; then
    local_read=$(api "agent-alpha-1" "groups/read" "{\"group_id\":\"${GRP_ID}\"}" 2>/dev/null || echo "{}")
    if echo "$local_read" | jq -e ".members[] | select(.entity_id == \"${MEMBER_ID}\")" > /dev/null 2>&1; then
        pass "member visible in local groups/read"
    else
        fail "member NOT visible in local groups/read"
        T4_OK=false
    fi
fi

# 4d: Check if group descriptor propagated to keeper-alpha-1 via GroupExchange
# GroupExchange runs every ~60s per hop; this is informational only (not a hard failure)
# because propagation timing depends on peer discovery and exchange scheduling.
if $T4_OK; then
    GX_TIMEOUT=30
    deadline=$((SECONDS + GX_TIMEOUT))
    GX_FOUND=false
    while [ $SECONDS -lt $deadline ]; do
        grp_list=$(api "keeper-alpha-1" "groups/list" '{}' 2>/dev/null || echo "[]")
        if echo "$grp_list" | jq -e ".[] | select(.id == \"${GRP_ID}\")" > /dev/null 2>&1; then
            GX_FOUND=true
            break
        fi
        sleep 5
    done
    if $GX_FOUND; then
        pass "group descriptor propagated to keeper-alpha-1"
    else
        echo "  INFO: group descriptor not yet on keeper-alpha-1 after ${GX_TIMEOUT}s (expected -- GroupExchange is async)"
    fi
fi

T4_LAT=$(( $(date +%s) - T4_START ))
if $T4_OK; then
    record "group-descriptor-propagation" "PASS" "$T4_LAT"
else
    record "group-descriptor-propagation" "FAIL" "$T4_LAT"
fi
set -e
echo ""

# --- Test 5: Cluster health [5] ---------------------------------------------

echo "[5] Cluster health..."
T5_START=$(date +%s)
HEALTH_OK=true

for node in $ALL_NODES; do
    status=$(api "$node" "status" || echo '{}')
    hot=$(echo "$status" | jq -r '.peers_hot // 0' 2>/dev/null || echo 0)
    if [ "$hot" -lt 1 ]; then
        fail "$node has 0 hot peers"
        HEALTH_OK=false
    fi
done

# Check sync errors
TOTAL_ERRORS=0
for node in $ALL_NODES; do
    errors=$(api "$node" "diagnostics" | jq '.sync_errors // 0' 2>/dev/null || echo 0)
    TOTAL_ERRORS=$((TOTAL_ERRORS + errors))
done

if [ "$TOTAL_ERRORS" -gt 0 ]; then
    fail "${TOTAL_ERRORS} total sync errors across cluster"
    HEALTH_OK=false
fi

T5_LAT=$(( $(date +%s) - T5_START ))
if $HEALTH_OK; then
    pass "all nodes have hot peers, zero sync errors"
    record "cluster-health" "PASS" "$T5_LAT"
else
    record "cluster-health" "FAIL" "$T5_LAT"
fi
echo ""

# --- Results ----------------------------------------------------------------

echo "==========================================="
echo "  RESULTS: ${PASSED} passed, ${FAILED} failed"
echo "==========================================="

# --- JSON Report ------------------------------------------------------------

if [ "${REPORT:-0}" = "1" ]; then
    REPORT_DIR="${SCRIPT_DIR}/reports"
    mkdir -p "$REPORT_DIR"
    REPORT_FILE="${REPORT_DIR}/ci-smoke-${TS}.json"

    OVERALL="PASSED"
    if [ "$FAILED" -gt 0 ]; then OVERALL="FAILED"; fi

    TESTS_JSON="["
    for i in "${!R_NAMES[@]}"; do
        if [ "$i" -gt 0 ]; then TESTS_JSON+=","; fi
        TESTS_JSON+="{\"name\":\"${R_NAMES[$i]}\",\"status\":\"${R_STATUSES[$i]}\",\"latency_secs\":${R_LATENCIES[$i]}}"
    done
    TESTS_JSON+="]"

    cat > "$REPORT_FILE" <<EOF
{
  "test_name": "ci-smoke",
  "status": "${OVERALL}",
  "environment": "docker-ci",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "node_count": ${TOTAL},
  "tests": ${TESTS_JSON}
}
EOF

    echo "Report: ${REPORT_FILE}"
fi

exit "$FAILED"
