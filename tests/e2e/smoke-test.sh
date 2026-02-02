#!/usr/bin/env bash
# Smoke test suite for cordelia zoned topology.
# Designed to run INSIDE the orchestrator container (direct hostname access).
# Can also run from the Docker host if DOCKER_EXEC=1 is set.
#
# Usage:
#   docker exec cordelia-e2e-orchestrator ./smoke-test.sh
#   ./smoke-test.sh  # from inside orchestrator

set -euo pipefail

BEARER="${BEARER_TOKEN:-test-token-fixed}"
PASSED=0
FAILED=0
SKIPPED=0

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
skip() { echo "  SKIP: $1"; SKIPPED=$((SKIPPED + 1)); }

wait_for_item() {
    local host="$1" item_id="$2" timeout="${3:-60}"
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

# --- Pre-flight -------------------------------------------------------------

echo "=== Cordelia Smoke Test Suite ==="
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

echo "[0] Pre-flight: checking node connectivity..."
for node in boot1 boot2 boot3; do
    if api "$node" "status" > /dev/null 2>&1; then
        pass "backbone $node reachable"
    else
        fail "backbone $node unreachable"
    fi
done

# Discover orgs from env or defaults
ORG_SPEC="${ORG_SPEC:-alpha:3:3:22,bravo:3:3:20,charlie:3:2:18}"
IFS=',' read -ra ORG_DEFS <<< "$ORG_SPEC"
ORG_NAMES=()
for def in "${ORG_DEFS[@]}"; do
    IFS=':' read -r name _ _ _ <<< "$def"
    ORG_NAMES+=("$name")
done

for org in "${ORG_NAMES[@]}"; do
    if api "edge-${org}-1" "status" > /dev/null 2>&1; then
        pass "edge-${org}-1 reachable"
    else
        fail "edge-${org}-1 unreachable"
    fi
done
echo ""

# --- Test 1: Cross-org replication ------------------------------------------

echo "[1] Cross-org replication (shared-xorg)..."
TS=$(date +%s)
XORG_ITEM="smoke-xorg-${TS}"

api "agent-${ORG_NAMES[0]}-1" "l2/write" \
    "{\"item_id\":\"${XORG_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"cross-org smoke\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-${ORG_NAMES[0]}-1\"}}" > /dev/null

if wait_for_item "keeper-${ORG_NAMES[1]}-1" "$XORG_ITEM" 90; then
    pass "shared-xorg item reached ${ORG_NAMES[1]} via backbone"
else
    fail "shared-xorg item did NOT reach ${ORG_NAMES[1]}"
fi

if wait_for_item "keeper-${ORG_NAMES[0]}-1" "$XORG_ITEM" 30; then
    pass "shared-xorg item reached ${ORG_NAMES[0]} keeper (intra-org)"
else
    fail "shared-xorg item did NOT reach ${ORG_NAMES[0]} keeper"
fi

# Third org if available
if [ ${#ORG_NAMES[@]} -ge 3 ]; then
    if wait_for_item "keeper-${ORG_NAMES[2]}-1" "$XORG_ITEM" 90; then
        pass "shared-xorg item reached ${ORG_NAMES[2]} (3rd org)"
    else
        fail "shared-xorg item did NOT reach ${ORG_NAMES[2]}"
    fi
fi
echo ""

# --- Test 2: Group isolation ------------------------------------------------

echo "[2] Group isolation (org-internal)..."
INTERNAL_ITEM="smoke-internal-${TS}"
ORG_A="${ORG_NAMES[0]}"
ORG_B="${ORG_NAMES[1]}"

api "agent-${ORG_A}-1" "l2/write" \
    "{\"item_id\":\"${INTERNAL_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"isolation smoke\"},\"meta\":{\"group_id\":\"${ORG_A}-internal\",\"author_id\":\"agent-${ORG_A}-1\"}}" > /dev/null

if wait_for_item "keeper-${ORG_A}-1" "$INTERNAL_ITEM" 60; then
    pass "${ORG_A}-internal item reached ${ORG_A} keeper"
else
    fail "${ORG_A}-internal item did NOT reach ${ORG_A} keeper"
fi

# Wait for potential leakage
sleep 15

if assert_no_item "keeper-${ORG_B}-1" "$INTERNAL_ITEM"; then
    pass "${ORG_A}-internal item correctly absent from ${ORG_B}"
else
    fail "${ORG_A}-internal item LEAKED to ${ORG_B}"
fi
echo ""

# --- Test 3: Reverse direction ----------------------------------------------

echo "[3] Reverse replication (${ORG_B} -> ${ORG_A})..."
REVERSE_ITEM="smoke-reverse-${TS}"

api "agent-${ORG_B}-1" "l2/write" \
    "{\"item_id\":\"${REVERSE_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"reverse smoke\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-${ORG_B}-1\"}}" > /dev/null

if wait_for_item "keeper-${ORG_A}-1" "$REVERSE_ITEM" 90; then
    pass "reverse item reached ${ORG_A} from ${ORG_B}"
else
    fail "reverse item did NOT reach ${ORG_A}"
fi
echo ""

# --- Test 4: Node health summary --------------------------------------------

echo "[4] Cluster health..."
TOTAL=0
HEALTHY=0
for node in boot1 boot2 boot3; do
    TOTAL=$((TOTAL + 1))
    status=$(api "$node" "status" || echo '{}')
    hot=$(echo "$status" | jq -r '.peers_hot // 0' 2>/dev/null || echo 0)
    if [ "$hot" -gt 0 ]; then HEALTHY=$((HEALTHY + 1)); fi
done

for org in "${ORG_NAMES[@]}"; do
    for i in 1 2; do
        TOTAL=$((TOTAL + 1))
        status=$(api "edge-${org}-${i}" "status" 2>/dev/null || echo '{}')
        hot=$(echo "$status" | jq -r '.peers_hot // 0' 2>/dev/null || echo 0)
        if [ "$hot" -gt 0 ]; then HEALTHY=$((HEALTHY + 1)); fi
    done
done

if [ "$HEALTHY" -ge "$((TOTAL * 80 / 100))" ]; then
    pass "cluster health: ${HEALTHY}/${TOTAL} key nodes have hot peers (>=80%)"
else
    fail "cluster health: only ${HEALTHY}/${TOTAL} key nodes have hot peers (<80%)"
fi
echo ""

# --- Results ----------------------------------------------------------------

echo "==========================================="
echo "  RESULTS: ${PASSED} passed, ${FAILED} failed, ${SKIPPED} skipped"
echo "==========================================="

exit "$FAILED"
