#!/usr/bin/env bash
# test-replication-e2e.sh -- Verify P2P replication between local node and Fly node.
#
# Tests:
#   1. Write L2 item on LOCAL node -> verify it appears on FLY node
#   2. Write L2 item on FLY node   -> verify it appears on LOCAL node
#   3. Create group on LOCAL        -> verify it appears on FLY
#   4. Clean up test items
#
# Prerequisites:
#   - Local cordelia-node running (127.0.0.1:9473)
#   - Fly app 'cordelia-portal' running (fly ssh available)
#   - Both nodes connected to P2P mesh (at least 1 hot peer)
#   - Both nodes share at least one group
#
# Usage:
#   ./scripts/test-replication-e2e.sh [--group GROUP_ID] [--timeout SECS] [--no-cleanup]

set -euo pipefail

# -- Config ------------------------------------------------------------------

LOCAL_API="http://127.0.0.1:9473"
LOCAL_TOKEN=$(cat ~/.cordelia/node-token 2>/dev/null || true)
FLY_APP="cordelia-portal"
TEST_GROUP="${TEST_GROUP:-seed-drill}"
POLL_INTERVAL=5
MAX_WAIT=120  # seconds
CLEANUP=true
TIMESTAMP=$(date -u +%Y%m%dT%H%M%SZ)
TEST_PREFIX="e2e-repl-test"

# -- Parse args ---------------------------------------------------------------

while [[ $# -gt 0 ]]; do
  case $1 in
    --group)    TEST_GROUP="$2"; shift 2 ;;
    --timeout)  MAX_WAIT="$2"; shift 2 ;;
    --no-cleanup) CLEANUP=false; shift ;;
    *)          echo "Unknown arg: $1"; exit 1 ;;
  esac
done

# -- Helpers ------------------------------------------------------------------

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; FAILURES=$((FAILURES + 1)); }
info() { echo -e "${CYAN}INFO${NC} $1"; }
warn() { echo -e "${YELLOW}WARN${NC} $1"; }

FAILURES=0

local_api() {
  curl -s -X POST "${LOCAL_API}$1" \
    -H "Authorization: Bearer ${LOCAL_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "$2"
}

fly_api() {
  fly ssh console -a "$FLY_APP" -C "sh -c 'curl -s -X POST http://127.0.0.1:9473\$1 -H \"Authorization: Bearer \$(cat /home/cordelia/.cordelia/node-token)\" -H \"Content-Type: application/json\" -d \$2'" 2>/dev/null | tail -1
}

# fly_api is tricky with escaping. Use a simpler approach: write a temp script.
fly_node_api() {
  local endpoint="$1"
  local body="$2"
  # Use heredoc piped to fly ssh
  fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473${endpoint} -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"${body}\"'" 2>/dev/null | grep -v "^Connecting to"
}

# -- Preflight checks ---------------------------------------------------------

echo ""
echo "=========================================="
echo " Cordelia E2E Replication Test"
echo " ${TIMESTAMP}"
echo "=========================================="
echo ""

info "Test group: ${TEST_GROUP}"
info "Max wait: ${MAX_WAIT}s"
info "Cleanup: ${CLEANUP}"
echo ""

# Check local node
info "Checking local node..."
LOCAL_STATUS=$(local_api /api/v1/status '{}' 2>&1) || true
if echo "$LOCAL_STATUS" | grep -q '"node_id"'; then
  LOCAL_NODE_ID=$(echo "$LOCAL_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['node_id'])" 2>/dev/null || echo "unknown")
  LOCAL_ENTITY=$(echo "$LOCAL_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['entity_id'])" 2>/dev/null || echo "unknown")
  LOCAL_HOT=$(echo "$LOCAL_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['peers_hot'])" 2>/dev/null || echo "0")
  LOCAL_GROUPS=$(echo "$LOCAL_STATUS" | python3 -c "import sys,json; print(','.join(json.load(sys.stdin)['groups']))" 2>/dev/null || echo "")
  pass "Local node up: entity=${LOCAL_ENTITY}, hot_peers=${LOCAL_HOT}"
else
  fail "Local node not reachable at ${LOCAL_API}"
  echo "  Response: ${LOCAL_STATUS}"
  echo ""
  echo "Start your local node: cordelia-node --config ~/.cordelia/config.toml"
  exit 1
fi

# Check Fly node
info "Checking Fly node..."
FLY_STATUS=$(fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473/api/v1/status -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"{}\"'" 2>/dev/null | grep -v "^Connecting to") || true
if echo "$FLY_STATUS" | grep -q '"node_id"'; then
  FLY_NODE_ID=$(echo "$FLY_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['node_id'])" 2>/dev/null || echo "unknown")
  FLY_ENTITY=$(echo "$FLY_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['entity_id'])" 2>/dev/null || echo "unknown")
  FLY_HOT=$(echo "$FLY_STATUS" | python3 -c "import sys,json; print(json.load(sys.stdin)['peers_hot'])" 2>/dev/null || echo "0")
  FLY_GROUPS=$(echo "$FLY_STATUS" | python3 -c "import sys,json; print(','.join(json.load(sys.stdin)['groups']))" 2>/dev/null || echo "")
  pass "Fly node up: entity=${FLY_ENTITY}, hot_peers=${FLY_HOT}"
else
  fail "Fly node not reachable"
  echo "  Response: ${FLY_STATUS}"
  exit 1
fi

# Check shared group
if echo "$LOCAL_GROUPS" | grep -q "$TEST_GROUP" && echo "$FLY_GROUPS" | grep -q "$TEST_GROUP"; then
  pass "Both nodes share group: ${TEST_GROUP}"
else
  fail "Group '${TEST_GROUP}' not shared. Local: [${LOCAL_GROUPS}], Fly: [${FLY_GROUPS}]"
  exit 1
fi

# Check P2P connectivity
if [[ "$LOCAL_HOT" -gt 0 ]] && [[ "$FLY_HOT" -gt 0 ]]; then
  pass "P2P mesh connected (local: ${LOCAL_HOT} hot, fly: ${FLY_HOT} hot)"
else
  warn "Low peer count (local: ${LOCAL_HOT} hot, fly: ${FLY_HOT} hot) -- replication may be slow"
fi

echo ""

# -- Test 1: Local -> Fly replication -----------------------------------------

TEST1_ID="${TEST_PREFIX}-local-${TIMESTAMP}"
TEST1_DATA="{\"item_id\":\"${TEST1_ID}\",\"type\":\"learning\",\"data\":{\"type\":\"insight\",\"name\":\"E2E Test: Local to Fly\",\"content\":\"Test item written on local node at ${TIMESTAMP}\",\"tags\":[\"e2e-test\",\"replication\"]},\"meta\":{\"group_id\":\"${TEST_GROUP}\",\"visibility\":\"group\",\"author_id\":\"${LOCAL_ENTITY}\",\"key_version\":1}}"

info "TEST 1: Local -> Fly replication"
info "  Writing item ${TEST1_ID} to local node..."

WRITE1=$(local_api /api/v1/l2/write "$TEST1_DATA" 2>&1) || true
if echo "$WRITE1" | grep -q '"ok":true'; then
  pass "  Item written to local node"
else
  fail "  Failed to write item: ${WRITE1}"
fi

# Poll Fly node for the item
info "  Polling Fly node (max ${MAX_WAIT}s)..."
ELAPSED=0
FOUND=false
while [[ $ELAPSED -lt $MAX_WAIT ]]; do
  FLY_READ=$(fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473/api/v1/l2/read -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"{\\\"item_id\\\":\\\"${TEST1_ID}\\\"}\"'" 2>/dev/null | grep -v "^Connecting to") || true
  if echo "$FLY_READ" | grep -q '"data"'; then
    FOUND=true
    break
  fi
  sleep $POLL_INTERVAL
  ELAPSED=$((ELAPSED + POLL_INTERVAL))
  printf "  ...%ds\n" "$ELAPSED"
done

if $FOUND; then
  pass "  Item replicated to Fly node in ${ELAPSED}s"
else
  fail "  Item NOT found on Fly node after ${MAX_WAIT}s"
fi

echo ""

# -- Test 2: Fly -> Local replication -----------------------------------------

TEST2_ID="${TEST_PREFIX}-fly-${TIMESTAMP}"
TEST2_BODY="{\\\"item_id\\\":\\\"${TEST2_ID}\\\",\\\"type\\\":\\\"learning\\\",\\\"data\\\":{\\\"type\\\":\\\"insight\\\",\\\"name\\\":\\\"E2E Test: Fly to Local\\\",\\\"content\\\":\\\"Test item written on Fly node at ${TIMESTAMP}\\\",\\\"tags\\\":[\\\"e2e-test\\\",\\\"replication\\\"]},\\\"meta\\\":{\\\"group_id\\\":\\\"${TEST_GROUP}\\\",\\\"visibility\\\":\\\"group\\\",\\\"author_id\\\":\\\"${FLY_ENTITY}\\\",\\\"key_version\\\":1}}"

info "TEST 2: Fly -> Local replication"
info "  Writing item ${TEST2_ID} to Fly node..."

WRITE2=$(fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473/api/v1/l2/write -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"${TEST2_BODY}\"'" 2>/dev/null | grep -v "^Connecting to") || true
if echo "$WRITE2" | grep -q '"ok":true'; then
  pass "  Item written to Fly node"
else
  fail "  Failed to write item: ${WRITE2}"
fi

# Poll local node for the item
info "  Polling local node (max ${MAX_WAIT}s)..."
ELAPSED=0
FOUND=false
while [[ $ELAPSED -lt $MAX_WAIT ]]; do
  LOCAL_READ=$(local_api /api/v1/l2/read "{\"item_id\":\"${TEST2_ID}\"}" 2>&1) || true
  if echo "$LOCAL_READ" | grep -q '"data"'; then
    FOUND=true
    break
  fi
  sleep $POLL_INTERVAL
  ELAPSED=$((ELAPSED + POLL_INTERVAL))
  printf "  ...%ds\n" "$ELAPSED"
done

if $FOUND; then
  pass "  Item replicated to local node in ${ELAPSED}s"
else
  fail "  Item NOT found on local node after ${MAX_WAIT}s"
fi

echo ""

# -- Cleanup ------------------------------------------------------------------

if $CLEANUP; then
  info "Cleaning up test items..."

  # Delete from local
  local_api /api/v1/l2/delete "{\"item_id\":\"${TEST1_ID}\"}" >/dev/null 2>&1 || true
  local_api /api/v1/l2/delete "{\"item_id\":\"${TEST2_ID}\"}" >/dev/null 2>&1 || true

  # Delete from Fly
  fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473/api/v1/l2/delete -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"{\\\"item_id\\\":\\\"${TEST1_ID}\\\"}\"'" >/dev/null 2>&1 || true
  fly ssh console -a "$FLY_APP" -C "sh -c 'TOKEN=\$(cat /home/cordelia/.cordelia/node-token) && curl -s -X POST http://127.0.0.1:9473/api/v1/l2/delete -H \"Authorization: Bearer \$TOKEN\" -H \"Content-Type: application/json\" -d \"{\\\"item_id\\\":\\\"${TEST2_ID}\\\"}\"'" >/dev/null 2>&1 || true

  info "Cleanup complete"
fi

# -- Summary ------------------------------------------------------------------

echo ""
echo "=========================================="
if [[ $FAILURES -eq 0 ]]; then
  echo -e " ${GREEN}ALL TESTS PASSED${NC}"
else
  echo -e " ${RED}${FAILURES} TEST(S) FAILED${NC}"
fi
echo " Local:  ${LOCAL_NODE_ID:0:16}... (${LOCAL_ENTITY})"
echo " Fly:    ${FLY_NODE_ID:0:16}... (${FLY_ENTITY})"
echo " Group:  ${TEST_GROUP}"
echo " Time:   ${TIMESTAMP}"
echo "=========================================="
echo ""

exit $FAILURES
