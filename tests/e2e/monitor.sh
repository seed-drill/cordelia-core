#!/usr/bin/env bash
# Monitor cordelia zoned Docker topology.
# Usage: ./monitor.sh [--watch] [--item ITEM_ID]
#
# Run from the Docker host (NOT inside a container) -- uses docker port
# to discover published ports. For in-container monitoring, use smoke-test.sh
# inside the orchestrator instead.
#
# Shows: node health, peer counts, group membership, relay learned groups,
# and optionally tracks item propagation across the network.

set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
BEARER_TOKEN="${BEARER_TOKEN:-test-token-fixed}"
WATCH=false
TRACK_ITEM=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --watch|-w) WATCH=true; shift ;;
        --item|-i) TRACK_ITEM="$2"; shift 2 ;;
        *) echo "Usage: $0 [--watch] [--item ITEM_ID]"; exit 1 ;;
    esac
done

# Discover running cordelia containers
discover_nodes() {
    docker ps --format '{{.Names}}' | grep '^cordelia-e2e-' | sort
}

# Query a node's API
api_call() {
    local container="$1"
    local endpoint="$2"
    local port
    port=$(docker port "$container" 9473 2>/dev/null | head -1 | cut -d: -f2)
    if [ -z "$port" ]; then
        echo '{"error":"no port"}'
        return
    fi
    curl -sf --max-time 2 \
        -X POST \
        -H "Authorization: Bearer ${BEARER_TOKEN}" \
        -H "Content-Type: application/json" \
        -d '{}' \
        "http://127.0.0.1:${port}/api/v1/${endpoint}" 2>/dev/null || echo '{"error":"timeout"}'
}

# Check if a node has an item
check_item() {
    local container="$1"
    local item_id="$2"
    local port
    port=$(docker port "$container" 9473 2>/dev/null | head -1 | cut -d: -f2)
    if [ -z "$port" ]; then
        echo "ERR"
        return
    fi
    local code
    code=$(curl -sf --max-time 2 -o /dev/null -w '%{http_code}' \
        -H "Authorization: Bearer ${BEARER_TOKEN}" \
        "http://127.0.0.1:${port}/api/v1/items/${item_id}" 2>/dev/null || echo "000")
    if [ "$code" = "200" ]; then
        echo "YES"
    elif [ "$code" = "404" ]; then
        echo "NO"
    else
        echo "ERR"
    fi
}

# Main monitoring function
run_monitor() {
    local nodes
    nodes=$(discover_nodes)
    local total
    total=$(echo "$nodes" | wc -l | tr -d ' ')

    echo "==========================================="
    echo "  CORDELIA NETWORK MONITOR"
    echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "  Nodes: ${total}"
    echo "==========================================="
    echo ""

    # Categorize nodes
    local backbone_relays="" edge_relays="" keepers="" agents=""
    local healthy=0 unhealthy=0

    while IFS= read -r container; do
        local name="${container#cordelia-e2e-}"
        local status
        status=$(api_call "$container" "status")

        if echo "$status" | grep -q '"error"'; then
            unhealthy=$((unhealthy + 1))
            continue
        fi
        healthy=$((healthy + 1))

        local entity
        entity=$(echo "$status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('entity_id','?'))" 2>/dev/null || echo "?")
        local hot
        hot=$(echo "$status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('peers_hot',0))" 2>/dev/null || echo "0")
        local warm
        warm=$(echo "$status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('peers_warm',0))" 2>/dev/null || echo "0")
        local groups
        groups=$(echo "$status" | python3 -c "import sys,json; g=json.load(sys.stdin).get('groups',[]); print(','.join(g) if g else '-')" 2>/dev/null || echo "?")
        local uptime
        uptime=$(echo "$status" | python3 -c "import sys,json; print(json.load(sys.stdin).get('uptime_secs',0))" 2>/dev/null || echo "0")

        local line
        line=$(printf "  %-24s hot=%-3s warm=%-3s groups=%-30s up=%ss" "$entity" "$hot" "$warm" "$groups" "$uptime")

        case "$name" in
            boot*) backbone_relays="${backbone_relays}${line}\n" ;;
            edge-*) edge_relays="${edge_relays}${line}\n" ;;
            keeper-*) keepers="${keepers}${line}\n" ;;
            agent-*) agents="${agents}${line}\n" ;;
        esac
    done <<< "$nodes"

    echo "Health: ${healthy} healthy, ${unhealthy} unreachable"
    echo ""

    if [ -n "$backbone_relays" ]; then
        echo "--- Backbone Relays (transparent) ---"
        echo -e "$backbone_relays"
    fi
    if [ -n "$edge_relays" ]; then
        echo "--- Edge Relays (dynamic) ---"
        echo -e "$edge_relays"
    fi
    if [ -n "$keepers" ]; then
        echo "--- Keepers ---"
        echo -e "$keepers"
    fi
    if [ -n "$agents" ]; then
        echo "--- Agents ---"
        echo -e "$agents"
    fi

    # Item tracking
    if [ -n "$TRACK_ITEM" ]; then
        echo "--- Item Propagation: ${TRACK_ITEM} ---"
        local has=0 missing=0 errors=0
        while IFS= read -r container; do
            local name="${container#cordelia-e2e-}"
            local result
            result=$(check_item "$container" "$TRACK_ITEM")
            case "$result" in
                YES) has=$((has + 1)) ;;
                NO) missing=$((missing + 1)) ;;
                ERR) errors=$((errors + 1)) ;;
            esac
        done <<< "$nodes"
        echo "  Present: ${has}  Missing: ${missing}  Error: ${errors}  Total: ${total}"
        echo ""
    fi
}

if $WATCH; then
    while true; do
        clear
        run_monitor
        sleep 10
    done
else
    run_monitor
fi
