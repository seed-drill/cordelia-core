#!/usr/bin/env bash
# Resilience tests: partition recovery and split-brain LWW resolution.
# Runs from the HOST (not inside orchestrator) -- needs docker network access.
#
# Requires: zoned topology running (ci or default).
#   cd tests/e2e && cp topology-ci.env topology.env && bash gen-compose-zoned.sh
#   docker compose -f docker-compose.generated.yml up -d --wait
#   sleep 20  # mesh convergence
#   bash resilience-test.sh
#
# Set REPORT=1 to emit JSON report to ./reports/.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "${SCRIPT_DIR}/lib/api.sh"
. "${SCRIPT_DIR}/lib/wait.sh"

BEARER_TOKEN="${BEARER_TOKEN:-test-token-fixed}"
TIMEOUT=60
TS=$(date +%s)
PASSED=0
FAILED=0

# In zoned topology, nodes are accessed by container hostname via Docker exec
# on the orchestrator, OR directly if running on the host with port mappings.
# For simplicity, use the orchestrator as a proxy for API calls.
ORCH="cordelia-e2e-orchestrator"

# API call routed through orchestrator container (has access to all hostnames)
zapi() {
    local host="$1" endpoint="$2" data="${3:-{}}"
    docker exec "$ORCH" curl -sf --max-time 5 \
        -X POST \
        -H "Authorization: Bearer ${BEARER_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "$data" \
        "http://${host}:9473/api/v1/${endpoint}" 2>/dev/null
}

pass() { echo "  PASS: $1"; PASSED=$((PASSED + 1)); }
fail() { echo "  FAIL: $1"; FAILED=$((FAILED + 1)); }

wait_for() {
    local host="$1" item_id="$2" timeout="${3:-$TIMEOUT}"
    local deadline=$((SECONDS + timeout))
    while [ "$SECONDS" -lt "$deadline" ]; do
        local result
        result=$(zapi "$host" "l2/read" "{\"item_id\":\"$item_id\"}" || echo "{}")
        if echo "$result" | grep -q '"data"'; then
            return 0
        fi
        sleep 3
    done
    return 1
}

# Diagnostics on failure
diag() {
    local nodes="$*"
    echo "  --- Diagnostics ---"
    for node in $nodes; do
        echo "  [$node]:"
        zapi "$node" "diagnostics" | jq -c '{peers_hot: .peers_hot, peers_warm: .peers_warm, sync_errors: .sync_errors, items_synced: .items_synced}' 2>/dev/null || echo "  unreachable"
    done
    echo "  ---"
}

# Timing records
declare -a R_NAMES=()
declare -a R_STATUSES=()
declare -a R_LATENCIES=()
record() { R_NAMES+=("$1"); R_STATUSES+=("$2"); R_LATENCIES+=("$3"); }

# --- Discover network names -------------------------------------------------

echo "=== Cordelia Resilience Tests ==="
echo "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# Find the backbone network (created by gen-compose-zoned.sh)
BACKBONE=$(docker network ls --filter name=backbone --format '{{.Name}}' | grep -E 'backbone$' | head -1)
if [ -z "$BACKBONE" ]; then
    echo "ABORT: backbone network not found. Is the zoned topology running?"
    exit 1
fi
echo "Backbone network: ${BACKBONE}"

# Verify key nodes are reachable
echo "[0] Pre-flight..."
for node in agent-alpha-1 keeper-alpha-1 keeper-bravo-1 edge-alpha-1; do
    if zapi "$node" "status" > /dev/null 2>&1; then
        pass "$node reachable"
    else
        fail "$node unreachable"
        echo "ABORT: critical node unreachable"
        exit 1
    fi
done
echo ""

# === Test 1: Partition Recovery ==============================================
#
# Topology: agent-alpha-1 -> edge-alpha-1 -> [backbone] -> edge-bravo-1 -> keeper-bravo-1
# Partition: disconnect edge-alpha-1 from backbone (isolates alpha org from bravo)
# Recovery: reconnect edge-alpha-1, verify anti-entropy delivers queued item

echo "[1] Partition recovery..."
T1_START=$(date +%s)
PRE_ITEM="resil-pre-${TS}"
PART_ITEM="resil-part-${TS}"

# 1a: Write pre-partition item, verify it reaches bravo
echo "  Writing pre-partition item..."
zapi "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${PRE_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"pre-partition\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

if wait_for "keeper-bravo-1" "$PRE_ITEM" "$TIMEOUT"; then
    pass "pre-partition item reached bravo"
else
    fail "pre-partition item did NOT reach bravo (baseline broken)"
    diag agent-alpha-1 keeper-bravo-1 edge-alpha-1 edge-bravo-1
    record "partition-recovery" "FAIL" "$(( $(date +%s) - T1_START ))"
    echo ""
    # Skip to test 2 -- don't exit, run all tests
    T1_SKIP=true
fi

if [ "${T1_SKIP:-false}" = "false" ]; then
    # 1b: Partition -- disconnect edge-alpha-1 from backbone
    echo "  Disconnecting edge-alpha-1 from backbone..."
    docker network disconnect "$BACKBONE" cordelia-e2e-edge-alpha-1

    # 1c: Write during partition
    echo "  Writing item during partition..."
    zapi "agent-alpha-1" "l2/write" \
        "{\"item_id\":\"${PART_ITEM}\",\"type\":\"learning\",\"data\":{\"test\":\"during-partition\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

    # 1d: Verify item reaches alpha keeper (still on same org network)
    if wait_for "keeper-alpha-1" "$PART_ITEM" 30; then
        pass "partition item reached alpha keeper (same org network)"
    else
        fail "partition item did NOT reach alpha keeper"
        diag agent-alpha-1 keeper-alpha-1 edge-alpha-1
    fi

    # 1e: Verify item has NOT reached bravo (partitioned)
    sleep 10
    result=$(zapi "keeper-bravo-1" "l2/read" "{\"item_id\":\"${PART_ITEM}\"}" || echo "{}")
    if echo "$result" | grep -q '"data"'; then
        fail "partition item leaked to bravo during partition"
    else
        pass "partition item correctly absent from bravo"
    fi

    # 1f: Reconnect
    echo "  Reconnecting edge-alpha-1 to backbone..."
    docker network connect "$BACKBONE" cordelia-e2e-edge-alpha-1

    # 1g: Wait for anti-entropy recovery
    echo "  Waiting for partition recovery..."
    if wait_for "keeper-bravo-1" "$PART_ITEM" "$TIMEOUT"; then
        T1_LAT=$(( $(date +%s) - T1_START ))
        pass "partition item recovered to bravo after reconnect (${T1_LAT}s)"
        record "partition-recovery" "PASS" "$T1_LAT"
    else
        T1_LAT=$(( $(date +%s) - T1_START ))
        fail "partition item did NOT recover to bravo after ${TIMEOUT}s"
        diag agent-alpha-1 keeper-bravo-1 edge-alpha-1 edge-bravo-1
        record "partition-recovery" "FAIL" "$T1_LAT"
    fi
fi
echo ""

# === Test 2: Split-Brain LWW Convergence =====================================
#
# Both sides write the same item_id during partition.
# After reconnect, LWW (updated_at) should converge to the later write.

echo "[2] Split-brain LWW convergence..."
T2_START=$(date +%s)
LWW_ITEM="resil-lww-${TS}"

# 2a: Write initial version on alpha
echo "  Writing initial version on alpha..."
zapi "agent-alpha-1" "l2/write" \
    "{\"item_id\":\"${LWW_ITEM}\",\"type\":\"learning\",\"data\":{\"version\":\"v1-alpha\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

# Wait for it to propagate to bravo (establishes baseline)
if ! wait_for "keeper-bravo-1" "$LWW_ITEM" "$TIMEOUT"; then
    fail "LWW baseline item did NOT reach bravo"
    diag agent-alpha-1 keeper-bravo-1 edge-alpha-1
    record "split-brain-lww" "FAIL" "$(( $(date +%s) - T2_START ))"
    echo ""
else
    # 2b: Partition
    echo "  Disconnecting edge-alpha-1 from backbone..."
    docker network disconnect "$BACKBONE" cordelia-e2e-edge-alpha-1

    # 2c: Write on alpha side (T2)
    sleep 1
    echo "  Writing alpha-side update during partition..."
    zapi "agent-alpha-1" "l2/write" \
        "{\"item_id\":\"${LWW_ITEM}\",\"type\":\"learning\",\"data\":{\"version\":\"v2-alpha-stale\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"agent-alpha-1\"}}" > /dev/null

    # 2d: Write on bravo side (T3 > T2, should win)
    sleep 2
    echo "  Writing bravo-side update during partition (later timestamp, should win)..."
    zapi "keeper-bravo-1" "l2/write" \
        "{\"item_id\":\"${LWW_ITEM}\",\"type\":\"learning\",\"data\":{\"version\":\"v3-bravo-wins\"},\"meta\":{\"group_id\":\"shared-xorg\",\"author_id\":\"keeper-bravo-1\"}}" > /dev/null

    # 2e: Reconnect
    echo "  Reconnecting edge-alpha-1 to backbone..."
    docker network connect "$BACKBONE" cordelia-e2e-edge-alpha-1

    # 2f: Wait for convergence, then check both sides have bravo's version
    echo "  Waiting for LWW convergence..."
    sleep 30  # allow anti-entropy to run

    LWW_OK=true

    # Check alpha side
    alpha_data=$(zapi "keeper-alpha-1" "l2/read" "{\"item_id\":\"${LWW_ITEM}\"}" || echo "{}")
    alpha_version=$(echo "$alpha_data" | jq -r '.data.version // empty' 2>/dev/null || echo "")
    if [ "$alpha_version" = "v3-bravo-wins" ]; then
        pass "alpha converged to bravo's version (LWW correct)"
    else
        # May need more time -- poll
        deadline=$((SECONDS + TIMEOUT))
        while [ "$SECONDS" -lt "$deadline" ]; do
            alpha_data=$(zapi "keeper-alpha-1" "l2/read" "{\"item_id\":\"${LWW_ITEM}\"}" || echo "{}")
            alpha_version=$(echo "$alpha_data" | jq -r '.data.version // empty' 2>/dev/null || echo "")
            if [ "$alpha_version" = "v3-bravo-wins" ]; then
                break
            fi
            sleep 3
        done
        if [ "$alpha_version" = "v3-bravo-wins" ]; then
            pass "alpha converged to bravo's version (LWW correct)"
        else
            fail "alpha has '${alpha_version}', expected 'v3-bravo-wins'"
            diag agent-alpha-1 keeper-alpha-1 edge-alpha-1
            LWW_OK=false
        fi
    fi

    # Check bravo side (should still have its own version)
    bravo_data=$(zapi "keeper-bravo-1" "l2/read" "{\"item_id\":\"${LWW_ITEM}\"}" || echo "{}")
    bravo_version=$(echo "$bravo_data" | jq -r '.data.version // empty' 2>/dev/null || echo "")
    if [ "$bravo_version" = "v3-bravo-wins" ]; then
        pass "bravo retained its version (LWW correct)"
    else
        fail "bravo has '${bravo_version}', expected 'v3-bravo-wins'"
        diag keeper-bravo-1 edge-bravo-1
        LWW_OK=false
    fi

    T2_LAT=$(( $(date +%s) - T2_START ))
    if $LWW_OK; then
        record "split-brain-lww" "PASS" "$T2_LAT"
    else
        record "split-brain-lww" "FAIL" "$T2_LAT"
    fi
fi
echo ""

# === Results =================================================================

echo "==========================================="
echo "  RESULTS: ${PASSED} passed, ${FAILED} failed"
echo "==========================================="

# --- JSON Report ------------------------------------------------------------

if [ "${REPORT:-0}" = "1" ]; then
    REPORT_DIR="${SCRIPT_DIR}/reports"
    mkdir -p "$REPORT_DIR"
    REPORT_FILE="${REPORT_DIR}/resilience-${TS}.json"

    OVERALL="PASSED"
    if [ "$FAILED" -gt 0 ]; then OVERALL="FAILED"; fi

    TESTS_JSON="["
    for i in "${!R_NAMES[@]}"; do
        if [ "$i" -gt 0 ]; then TESTS_JSON+=","; fi
        TESTS_JSON+="{\"name\":\"${R_NAMES[$i]}\",\"status\":\"${R_STATUSES[$i]}\",\"latency_secs\":${R_LATENCIES[$i]}}"
    done
    TESTS_JSON+="]"

    # Cluster diagnostics on failure
    DIAG_JSON="null"
    if [ "$FAILED" -gt 0 ]; then
        DIAG_JSON="{"
        first=true
        for node in boot1 boot2 edge-alpha-1 edge-bravo-1 keeper-alpha-1 keeper-bravo-1 agent-alpha-1; do
            diag_data=$(zapi "$node" "diagnostics" 2>/dev/null || echo '"unreachable"')
            diag_escaped=$(echo "$diag_data" | jq -c . 2>/dev/null || echo '"unreachable"')
            if $first; then first=false; else DIAG_JSON+=","; fi
            DIAG_JSON+="\"${node}\":${diag_escaped}"
        done
        DIAG_JSON+="}"
    fi

    cat > "$REPORT_FILE" <<EOF
{
  "test_name": "resilience",
  "status": "${OVERALL}",
  "environment": "docker-ci",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "tests": ${TESTS_JSON},
  "diagnostics": ${DIAG_JSON}
}
EOF

    echo "Report: ${REPORT_FILE}"
fi

exit "$FAILED"
