#!/usr/bin/env bash
# Generate docker-compose with realistic zoned network topology.
#
# Topology:
#   [keepers + personals] -- [edge relays] -- [backbone relays] -- [edge relays] -- [keepers + personals]
#                                                  |
#                                          [backbone personals]
#
# Node roles:
#   boot*            - backbone relay (internet-facing, role=relay)
#   edge-{org}-*     - org edge relay (bridges org<->backbone, role=relay)
#   keeper-{org}-*   - secret keeper/archive (org private only, role=keeper)
#   agent-{org}-*    - personal node inside org (org private only, role=personal)
#   agent-bb-*       - personal node on backbone (no org, role=personal)
#
# Configuration: reads topology.env from same directory, then env var overrides.
#
# Usage:
#   ./gen-compose-zoned.sh                              # default topology
#   BACKBONE_COUNT=5 ./gen-compose-zoned.sh             # override
#   ORG_SPEC="a:3:2:5,b:2:1:3" ./gen-compose-zoned.sh  # custom orgs

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="$DIR/generated"
COMPOSE_FILE="$DIR/docker-compose.generated.yml"

# Save env var overrides, source topology.env, restore overrides.
# This ensures: CLI env vars > topology.env > hardcoded defaults.
_save_BACKBONE_COUNT="${BACKBONE_COUNT-}"
_save_BACKBONE_PERSONAL="${BACKBONE_PERSONAL-}"
_save_ORG_SPEC="${ORG_SPEC-}"
_save_IMAGE="${IMAGE-}"
_save_BEARER_TOKEN="${BEARER_TOKEN-}"
_save_RUST_LOG_LEVEL="${RUST_LOG_LEVEL-}"
_save_GOV_HOT_MIN="${GOV_HOT_MIN-}"
_save_GOV_HOT_MAX="${GOV_HOT_MAX-}"
_save_GOV_WARM_MIN="${GOV_WARM_MIN-}"
_save_GOV_WARM_MAX="${GOV_WARM_MAX-}"
_save_GOV_COLD_MAX="${GOV_COLD_MAX-}"
_save_SYNC_MODERATE="${SYNC_MODERATE-}"
_save_SYNC_TACITURN="${SYNC_TACITURN-}"

if [ -f "$DIR/topology.env" ]; then
    set -a
    # shellcheck source=topology.env
    source "$DIR/topology.env"
    set +a
fi

# Restore any CLI overrides
[ -n "$_save_BACKBONE_COUNT" ] && BACKBONE_COUNT="$_save_BACKBONE_COUNT"
[ -n "$_save_BACKBONE_PERSONAL" ] && BACKBONE_PERSONAL="$_save_BACKBONE_PERSONAL"
[ -n "$_save_ORG_SPEC" ] && ORG_SPEC="$_save_ORG_SPEC"
[ -n "$_save_IMAGE" ] && IMAGE="$_save_IMAGE"
[ -n "$_save_BEARER_TOKEN" ] && BEARER_TOKEN="$_save_BEARER_TOKEN"
[ -n "$_save_RUST_LOG_LEVEL" ] && RUST_LOG_LEVEL="$_save_RUST_LOG_LEVEL"
[ -n "$_save_GOV_HOT_MIN" ] && GOV_HOT_MIN="$_save_GOV_HOT_MIN"
[ -n "$_save_GOV_HOT_MAX" ] && GOV_HOT_MAX="$_save_GOV_HOT_MAX"
[ -n "$_save_GOV_WARM_MIN" ] && GOV_WARM_MIN="$_save_GOV_WARM_MIN"
[ -n "$_save_GOV_WARM_MAX" ] && GOV_WARM_MAX="$_save_GOV_WARM_MAX"
[ -n "$_save_GOV_COLD_MAX" ] && GOV_COLD_MAX="$_save_GOV_COLD_MAX"
[ -n "$_save_SYNC_MODERATE" ] && SYNC_MODERATE="$_save_SYNC_MODERATE"
[ -n "$_save_SYNC_TACITURN" ] && SYNC_TACITURN="$_save_SYNC_TACITURN"

# Apply hardcoded defaults for anything still unset
BACKBONE_COUNT="${BACKBONE_COUNT:-3}"
BACKBONE_PERSONAL="${BACKBONE_PERSONAL:-0}"
ORG_SPEC="${ORG_SPEC:-alpha:2:2:2,bravo:2:2:1,charlie:1:1:0}"
IMAGE="${IMAGE:-cordelia-node:test}"
BEARER_TOKEN="${BEARER_TOKEN:-test-token-fixed}"
RUST_LOG_LEVEL="${RUST_LOG_LEVEL:-warn}"

GOV_HOT_MIN="${GOV_HOT_MIN:-2}"
GOV_HOT_MAX="${GOV_HOT_MAX:-20}"
GOV_WARM_MIN="${GOV_WARM_MIN:-5}"
GOV_WARM_MAX="${GOV_WARM_MAX:-50}"
GOV_COLD_MAX="${GOV_COLD_MAX:-100}"
SYNC_MODERATE="${SYNC_MODERATE:-10}"
SYNC_TACITURN="${SYNC_TACITURN:-30}"

mkdir -p "$OUT_DIR"

# ============================================================================
# Parse org spec into arrays
# Format: name:edges:keepers[:personals]
# ============================================================================
declare -a ORG_NAMES ORG_EDGES ORG_KEEPERS ORG_PERSONALS

IFS=',' read -ra ORG_DEFS <<< "$ORG_SPEC"
for def in "${ORG_DEFS[@]}"; do
    IFS=':' read -r name edges keepers personals <<< "$def"
    ORG_NAMES+=("$name")
    ORG_EDGES+=("${edges:-2}")
    ORG_KEEPERS+=("${keepers:-2}")
    ORG_PERSONALS+=("${personals:-0}")
done

ORG_COUNT=${#ORG_NAMES[@]}

# ============================================================================
# Count totals
# ============================================================================
TOTAL_EDGES=0
TOTAL_KEEPERS=0
TOTAL_ORG_PERSONAL=0

for o in $(seq 0 $((ORG_COUNT - 1))); do
    TOTAL_EDGES=$((TOTAL_EDGES + ORG_EDGES[$o]))
    TOTAL_KEEPERS=$((TOTAL_KEEPERS + ORG_KEEPERS[$o]))
    TOTAL_ORG_PERSONAL=$((TOTAL_ORG_PERSONAL + ORG_PERSONALS[$o]))
done

TOTAL=$((BACKBONE_COUNT + BACKBONE_PERSONAL + TOTAL_EDGES + TOTAL_KEEPERS + TOTAL_ORG_PERSONAL))

echo "=== Zoned Topology ==="
echo "  Backbone relays:   ${BACKBONE_COUNT}"
echo "  Backbone personal: ${BACKBONE_PERSONAL}"
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    echo "  Org ${org}:          ${ORG_EDGES[$o]} edges, ${ORG_KEEPERS[$o]} keepers, ${ORG_PERSONALS[$o]} personal"
done
echo "  Total:             ${TOTAL} nodes"
echo ""

# ============================================================================
# Helper: generate config TOML for a node
# ============================================================================
gen_config() {
    local hostname="$1"
    local role="$2"       # relay, keeper, or personal
    local bootnodes="$3"  # newline-separated bootnode TOML blocks
    local trusted="$4"    # newline-separated trusted_relays TOML blocks (keeper only)

    cat > "$OUT_DIR/config-${hostname}.toml" <<TOML
[node]
identity_key = "/home/cordelia/.cordelia/node.key"
api_transport = "http"
api_addr = "0.0.0.0:9473"
database = "/home/cordelia/.cordelia/cordelia.db"
entity_id = "${hostname}"
role = "${role}"

[network]
listen_addr = "0.0.0.0:9474"

${bootnodes}

${trusted}

[governor]
hot_min = ${GOV_HOT_MIN}
hot_max = ${GOV_HOT_MAX}
warm_min = ${GOV_WARM_MIN}
warm_max = ${GOV_WARM_MAX}
cold_max = ${GOV_COLD_MAX}

[replication]
sync_interval_moderate_secs = ${SYNC_MODERATE}
sync_interval_taciturn_secs = ${SYNC_TACITURN}
tombstone_retention_days = 7
max_batch_size = 100
TOML
}

# ============================================================================
# Helper: compose service block
# ============================================================================
API_PORT_COUNTER=9473

gen_service() {
    local hostname="$1"
    local networks="$2"  # comma-separated network names

    local api_port=$API_PORT_COUNTER
    API_PORT_COUNTER=$((API_PORT_COUNTER + 10))

    cat >> "$COMPOSE_FILE" <<EOF
  ${hostname}:
    image: ${IMAGE}
    hostname: ${hostname}
    container_name: cordelia-e2e-${hostname}
    volumes:
      - ${OUT_DIR}/config-${hostname}.toml:/home/cordelia/.cordelia/config.toml:ro
    ports:
      - "${api_port}:9473"
    environment:
      - RUST_LOG=cordelia_node=${RUST_LOG_LEVEL},cordelia_api=${RUST_LOG_LEVEL}
    healthcheck:
      test: ["CMD", "curl", "-sf", "-X", "POST", "-H", "Authorization: Bearer ${BEARER_TOKEN}", "-H", "Content-Type: application/json", "-d", "{}", "http://127.0.0.1:9473/api/v1/status"]
      interval: 5s
      timeout: 3s
      retries: 30
    networks:
EOF

    IFS=',' read -ra nets <<< "$networks"
    for net in "${nets[@]}"; do
        echo "      - ${net}" >> "$COMPOSE_FILE"
    done
    echo "" >> "$COMPOSE_FILE"
}

# ============================================================================
# Helper: build bootnode TOML for backbone nodes
# ============================================================================
backbone_bootnodes_except() {
    local exclude="$1"
    local result=""
    for i in $(seq 1 "$BACKBONE_COUNT"); do
        if [ "$i" -ne "$exclude" ]; then
            result="${result}
[[network.bootnodes]]
addr = \"boot${i}:9474\""
        fi
    done
    echo "$result"
}

backbone_bootnodes_all() {
    local result=""
    for i in $(seq 1 "$BACKBONE_COUNT"); do
        result="${result}
[[network.bootnodes]]
addr = \"boot${i}:9474\""
    done
    echo "$result"
}

org_edge_bootnodes() {
    local org="$1"
    local org_idx="$2"
    local result=""
    for e in $(seq 1 "${ORG_EDGES[$org_idx]}"); do
        result="${result}
[[network.bootnodes]]
addr = \"edge-${org}-${e}:9474\""
    done
    echo "$result"
}

org_edge_trusted() {
    local org="$1"
    local org_idx="$2"
    local result=""
    for e in $(seq 1 "${ORG_EDGES[$org_idx]}"); do
        result="${result}
[[network.trusted_relays]]
addr = \"edge-${org}-${e}:9474\""
    done
    echo "$result"
}

# ============================================================================
# Generate configs
# ============================================================================

# Backbone relays -- bootnode to each other
for i in $(seq 1 "$BACKBONE_COUNT"); do
    gen_config "boot${i}" "relay" "$(backbone_bootnodes_except "$i")" ""
done

# Backbone personal nodes -- bootnode to all backbone relays
for i in $(seq 1 "$BACKBONE_PERSONAL"); do
    gen_config "agent-bb-${i}" "personal" "$(backbone_bootnodes_all)" ""
done

# Per-org: edge relays -- bootnode to backbone
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for e in $(seq 1 "${ORG_EDGES[$o]}"); do
        gen_config "edge-${org}-${e}" "relay" "$(backbone_bootnodes_all)" ""
    done
done

# Per-org: keepers -- bootnode to org edges, trusted relays = org edges
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for k in $(seq 1 "${ORG_KEEPERS[$o]}"); do
        gen_config "keeper-${org}-${k}" "keeper" \
            "$(org_edge_bootnodes "$org" "$o")" \
            "$(org_edge_trusted "$org" "$o")"
    done
done

# Per-org: personal nodes -- bootnode to org edges
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for p in $(seq 1 "${ORG_PERSONALS[$o]}"); do
        gen_config "agent-${org}-${p}" "personal" \
            "$(org_edge_bootnodes "$org" "$o")" ""
    done
done

# ============================================================================
# Generate docker-compose.generated.yml
# ============================================================================
cat > "$COMPOSE_FILE" <<EOF
# Auto-generated by gen-compose-zoned.sh
# Topology: ${BACKBONE_COUNT} backbone, ${BACKBONE_PERSONAL} bb-personal, ${ORG_COUNT} orgs, ${TOTAL} total
# Do not edit -- regenerate with: ./gen-compose-zoned.sh

services:
EOF

# Backbone relays
for i in $(seq 1 "$BACKBONE_COUNT"); do
    gen_service "boot${i}" "backbone"
done

# Backbone personal nodes
for i in $(seq 1 "$BACKBONE_PERSONAL"); do
    gen_service "agent-bb-${i}" "backbone"
done

# Edge relays (backbone + org)
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for e in $(seq 1 "${ORG_EDGES[$o]}"); do
        gen_service "edge-${org}-${e}" "backbone,org-${org}"
    done
done

# Keepers (org only)
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for k in $(seq 1 "${ORG_KEEPERS[$o]}"); do
        gen_service "keeper-${org}-${k}" "org-${org}"
    done
done

# Org personal nodes (org only)
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    for p in $(seq 1 "${ORG_PERSONALS[$o]}"); do
        gen_service "agent-${org}-${p}" "org-${org}"
    done
done

# Networks
cat >> "$COMPOSE_FILE" <<EOF
networks:
  backbone:
    driver: bridge
EOF

for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    cat >> "$COMPOSE_FILE" <<EOF
  org-${org}:
    driver: bridge
EOF
done

echo ""
echo "Generated ${TOTAL} node configs in ${OUT_DIR}/"
echo "Generated ${COMPOSE_FILE}"
echo ""
echo "Topology map:"
echo "  [backbone]           boot1..boot${BACKBONE_COUNT}"
if [ "$BACKBONE_PERSONAL" -gt 0 ]; then
    echo "  [backbone]           agent-bb-1..agent-bb-${BACKBONE_PERSONAL} (personal)"
fi
for o in $(seq 0 $((ORG_COUNT - 1))); do
    org="${ORG_NAMES[$o]}"
    echo "  [backbone+org-${org}] edge-${org}-1..edge-${org}-${ORG_EDGES[$o]} (relay)"
    echo "  [org-${org}]          keeper-${org}-1..keeper-${org}-${ORG_KEEPERS[$o]} (keeper)"
    if [ "${ORG_PERSONALS[$o]}" -gt 0 ]; then
        echo "  [org-${org}]          agent-${org}-1..agent-${org}-${ORG_PERSONALS[$o]} (personal)"
    fi
done
echo ""
echo "Run with: docker compose -f ${COMPOSE_FILE} up -d --wait"
