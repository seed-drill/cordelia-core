#!/usr/bin/env bash
# Polling helpers for e2e tests.
# Source this file: . lib/wait.sh

# Requires: lib/api.sh sourced first.

# wait_all_healthy COUNT TIMEOUT_SECS
# Wait until all N containers respond to /api/v1/status.
wait_all_healthy() {
    local count="$1" timeout="${2:-120}"
    local base_port=9473
    local deadline=$((SECONDS + timeout))

    echo "Waiting for ${count} nodes to become healthy..."
    while [ "$SECONDS" -lt "$deadline" ]; do
        local healthy=0
        for i in $(seq 1 "$count"); do
            local port=$((base_port + (i - 1) * 10))
            if api_status "127.0.0.1:${port}" > /dev/null 2>&1; then
                healthy=$((healthy + 1))
            fi
        done
        if [ "$healthy" -ge "$count" ]; then
            echo "All ${count} nodes healthy"
            return 0
        fi
        sleep 2
    done
    echo "TIMEOUT: only ${healthy}/${count} nodes healthy after ${timeout}s"
    return 1
}

# wait_hot_peers HOST MIN_HOT TIMEOUT_SECS
# Wait until a node has >= MIN_HOT hot peers.
wait_hot_peers() {
    local host="$1" min_hot="$2" timeout="${3:-120}"
    local deadline=$((SECONDS + timeout))

    while [ "$SECONDS" -lt "$deadline" ]; do
        local hot
        hot=$(hot_peer_count "$host")
        if [ "$hot" -ge "$min_hot" ]; then
            echo "Node ${host}: ${hot} hot peers (>= ${min_hot})"
            return 0
        fi
        sleep 2
    done
    local final_hot
    final_hot=$(hot_peer_count "$host")
    echo "TIMEOUT: ${host} has ${final_hot} hot peers, needed ${min_hot}"
    return 1
}

# wait_all_hot COUNT MIN_HOT TIMEOUT_SECS
# Wait until all N nodes have >= MIN_HOT hot peers.
wait_all_hot() {
    local count="$1" min_hot="$2" timeout="${3:-120}"
    local base_port=9473

    echo "Waiting for all ${count} nodes to have >= ${min_hot} hot peers..."
    for i in $(seq 1 "$count"); do
        local port=$((base_port + (i - 1) * 10))
        wait_hot_peers "127.0.0.1:${port}" "$min_hot" "$timeout" || return 1
    done
    echo "All nodes converged"
    return 0
}

# wait_item_replicated HOST ITEM_ID TIMEOUT_SECS
# Wait until an item is readable on a host.
wait_item_replicated() {
    local host="$1" item_id="$2" timeout="${3:-60}"
    local deadline=$((SECONDS + timeout))

    while [ "$SECONDS" -lt "$deadline" ]; do
        local result
        result=$(api_read_item "$host" "$item_id" 2>/dev/null || echo "{}")
        if echo "$result" | jq -e '.id // .item_id' > /dev/null 2>&1; then
            echo "Item ${item_id} found on ${host}"
            return 0
        fi
        sleep 2
    done
    echo "TIMEOUT: item ${item_id} not found on ${host} after ${timeout}s"
    return 1
}

# assert_item_absent HOST ITEM_ID WAIT_SECS
# Wait WAIT_SECS and verify item is NOT present.
assert_item_absent() {
    local host="$1" item_id="$2" wait="${3:-10}"

    sleep "$wait"
    local result
    result=$(api_read_item "$host" "$item_id" 2>/dev/null || echo "{}")
    if echo "$result" | jq -e '.id // .item_id' > /dev/null 2>&1; then
        echo "FAIL: item ${item_id} found on ${host} (should be absent)"
        return 1
    fi
    echo "OK: item ${item_id} absent on ${host}"
    return 0
}

# wait_group_has_member HOST GROUP_ID ENTITY_ID TIMEOUT_SECS
# Polls /groups/read, checks .members[] for matching entity_id.
wait_group_has_member() {
    local host="$1" group_id="$2" entity_id="$3" timeout="${4:-30}"
    local deadline=$((SECONDS + timeout))

    while [ "$SECONDS" -lt "$deadline" ]; do
        local result
        result=$(api_read_group "$host" "$group_id" 2>/dev/null || echo "{}")
        if echo "$result" | jq -e ".members[] | select(.entity_id == \"${entity_id}\")" > /dev/null 2>&1; then
            echo "Member ${entity_id} found in group ${group_id} on ${host}"
            return 0
        fi
        sleep 3
    done
    echo "TIMEOUT: member ${entity_id} not found in group ${group_id} on ${host} after ${timeout}s"
    return 1
}

# check_zero_sync_errors COUNT
# Verify no sync errors across all nodes.
check_zero_sync_errors() {
    local count="$1"
    local base_port=9473
    local total_errors=0

    for i in $(seq 1 "$count"); do
        local port=$((base_port + (i - 1) * 10))
        local errors
        errors=$(api_diag "127.0.0.1:${port}" | jq '.sync_errors // 0' 2>/dev/null || echo 0)
        if [ "$errors" -gt 0 ]; then
            echo "WARNING: node${i} has ${errors} sync errors"
            total_errors=$((total_errors + errors))
        fi
    done

    if [ "$total_errors" -gt 0 ]; then
        echo "FAIL: ${total_errors} total sync errors"
        return 1
    fi
    echo "OK: zero sync errors across ${count} nodes"
    return 0
}
