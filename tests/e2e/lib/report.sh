#!/usr/bin/env bash
# E2E test reporting library.
# Source with REPORT=1 to enable timestamped report generation.
# All functions are no-ops when REPORT != 1.
#
# Usage:
#   REPORT=1
#   . lib/report.sh
#   report_init "my-test" 341
#   report_snapshot "pre"
#   report_phase_start "phase-1"
#   ...
#   report_phase_end "phase-1" "PASS" "all good"
#   report_finalize $?

# Requires: lib/api.sh sourced first (for api_diag, api_peers).

# Guard: all functions are no-ops unless REPORT=1
_report_enabled() { [ "${REPORT:-0}" = "1" ]; }

# --- State -------------------------------------------------------------------

REPORT_DIR=""
_REPORT_TEST_NAME=""
_REPORT_NODE_COUNT=0
_REPORT_START=""
_REPORT_START_EPOCH=0
_REPORT_SAMPLER_PID=""
_REPORT_PHASES_FILE=""
_REPORT_TESTS_FILE=""
_REPORT_ITEMS_FILE=""
_REPORT_LATENCY_FILE=""

# --- Node discovery ----------------------------------------------------------
# Build list of all node hostnames from ORG_SPEC + BACKBONE_COUNT.
# Works inside orchestrator container where hostnames resolve via Docker DNS.

_report_all_nodes() {
    local nodes=""
    local bb_count="${BACKBONE_COUNT:-3}"

    # Backbone relays
    for i in $(seq 1 "$bb_count"); do
        nodes="${nodes} boot${i}"
    done

    # Backbone personal nodes
    local bb_personal="${BACKBONE_PERSONAL:-0}"
    for i in $(seq 1 "$bb_personal"); do
        nodes="${nodes} agent-bb-${i}"
    done

    # Parse ORG_SPEC
    local org_spec="${ORG_SPEC:-alpha:2:2:2,bravo:2:2:1,charlie:1:1:0}"
    IFS=',' read -ra org_defs <<< "$org_spec"
    for def in "${org_defs[@]}"; do
        IFS=':' read -r name edges keepers personals <<< "$def"
        edges="${edges:-2}"
        keepers="${keepers:-2}"
        personals="${personals:-0}"
        for e in $(seq 1 "$edges"); do
            nodes="${nodes} edge-${name}-${e}"
        done
        for k in $(seq 1 "$keepers"); do
            nodes="${nodes} keeper-${name}-${k}"
        done
        for p in $(seq 1 "$personals"); do
            nodes="${nodes} agent-${name}-${p}"
        done
    done

    echo "$nodes"
}

# --- Parallel diagnostics collection ----------------------------------------

_report_collect_diag() {
    local label="$1"
    local out_dir="$REPORT_DIR/snapshots"
    local tmp_dir
    tmp_dir=$(mktemp -d)
    local nodes
    nodes=$(_report_all_nodes)

    local pids=""
    local count=0
    local max_parallel=50

    for node in $nodes; do
        (
            local result
            result=$(curl -sf --max-time 5 -X POST \
                -H "Authorization: Bearer ${BEARER_TOKEN:-test-token-fixed}" \
                -H "Content-Type: application/json" \
                -d '{}' "http://${node}:9473/api/v1/diagnostics" 2>/dev/null || echo '{}')
            # Inject node hostname
            echo "$result" | jq --arg n "$node" '. + {node: $n}' > "$tmp_dir/${node}.json" 2>/dev/null || \
                echo "{\"node\":\"$node\",\"error\":\"unreachable\"}" > "$tmp_dir/${node}.json"
        ) &
        pids="$pids $!"
        count=$((count + 1))
        if [ "$count" -ge "$max_parallel" ]; then
            wait $pids 2>/dev/null || true
            pids=""
            count=0
        fi
    done
    wait $pids 2>/dev/null || true

    # Merge all node JSONs into array
    jq -s '.' "$tmp_dir"/*.json > "$out_dir/${label}.json" 2>/dev/null || echo "[]" > "$out_dir/${label}.json"
    rm -rf "$tmp_dir"
}

# --- Core functions ----------------------------------------------------------

# report_init TEST_NAME NODE_COUNT
report_init() {
    _report_enabled || return 0
    _REPORT_TEST_NAME="$1"
    _REPORT_NODE_COUNT="${2:-0}"
    _REPORT_START=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    _REPORT_START_EPOCH=$(date +%s)

    local ts
    ts=$(date -u +%Y-%m-%d-%H%M%S)
    REPORT_DIR="/tests/reports/${ts}-${_REPORT_TEST_NAME}"
    mkdir -p "$REPORT_DIR/snapshots"

    # Init tracking files
    _REPORT_PHASES_FILE="$REPORT_DIR/.phases.jsonl"
    _REPORT_TESTS_FILE="$REPORT_DIR/.tests.jsonl"
    _REPORT_ITEMS_FILE="$REPORT_DIR/.items.jsonl"
    _REPORT_LATENCY_FILE="$REPORT_DIR/.latency.jsonl"
    : > "$_REPORT_PHASES_FILE"
    : > "$_REPORT_TESTS_FILE"
    : > "$_REPORT_ITEMS_FILE"
    : > "$_REPORT_LATENCY_FILE"

    echo "Report: ${REPORT_DIR}"
}

# report_snapshot LABEL
report_snapshot() {
    _report_enabled || return 0
    local label="$1"
    echo "  [report] Collecting ${label} snapshot..."
    _report_collect_diag "$label"
}

# report_phase_start PHASE_NAME
report_phase_start() {
    _report_enabled || return 0
    local name="$1"
    local now
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local epoch
    epoch=$(date +%s)
    # Store start time in temp var for this phase
    eval "_REPORT_PHASE_START_${name//[^a-zA-Z0-9]/_}=$epoch"
    eval "_REPORT_PHASE_ISO_${name//[^a-zA-Z0-9]/_}=$now"
}

# report_phase_end PHASE_NAME [STATUS] [DETAIL]
report_phase_end() {
    _report_enabled || return 0
    local name="$1"
    local status="${2:-PASS}"
    local detail="${3:-}"
    local now
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local epoch
    epoch=$(date +%s)
    local safe_name="${name//[^a-zA-Z0-9]/_}"
    local start_epoch
    eval "start_epoch=\${_REPORT_PHASE_START_${safe_name}:-$epoch}"
    local start_iso
    eval "start_iso=\${_REPORT_PHASE_ISO_${safe_name}:-$now}"
    local duration=$((epoch - start_epoch))

    jq -n --arg n "$name" --arg s "$status" --arg si "$start_iso" \
        --arg ei "$now" --argjson d "$duration" --arg dt "$detail" \
        '{name:$n, status:$s, start:$si, end:$ei, duration_secs:$d, detail:$dt}' \
        >> "$_REPORT_PHASES_FILE"
}

# report_start_sampler [INTERVAL_SECS]
report_start_sampler() {
    _report_enabled || return 0
    local interval="${1:-10}"
    local samples_file="$REPORT_DIR/samples.jsonl"

    (
        while true; do
            local nodes
            nodes=$(_report_all_nodes)
            local total_hot=0
            local nodes_with_hot=0
            local node_count=0
            local tmp_dir
            tmp_dir=$(mktemp -d)

            # Parallel peer collection
            local pids=""
            local pc=0
            for node in $nodes; do
                (
                    local hot
                    hot=$(curl -sf --max-time 5 -X POST \
                        -H "Authorization: Bearer ${BEARER_TOKEN:-test-token-fixed}" \
                        -H "Content-Type: application/json" \
                        -d '{}' "http://${node}:9473/api/v1/peers" 2>/dev/null \
                        | jq '.hot // 0' 2>/dev/null || echo "0")
                    echo "$hot" > "$tmp_dir/${node}"
                ) &
                pids="$pids $!"
                pc=$((pc + 1))
                if [ "$pc" -ge 50 ]; then
                    wait $pids 2>/dev/null || true
                    pids=""
                    pc=0
                fi
            done
            wait $pids 2>/dev/null || true

            for node in $nodes; do
                node_count=$((node_count + 1))
                local hot=0
                [ -f "$tmp_dir/${node}" ] && hot=$(cat "$tmp_dir/${node}")
                total_hot=$((total_hot + hot))
                [ "$hot" -gt 0 ] && nodes_with_hot=$((nodes_with_hot + 1))
            done
            rm -rf "$tmp_dir"

            local avg_hot="0"
            if [ "$node_count" -gt 0 ]; then
                avg_hot=$(echo "scale=2; $total_hot / $node_count" | bc 2>/dev/null || echo "0")
            fi
            local elapsed=$(( $(date +%s) - _REPORT_START_EPOCH ))

            jq -n --argjson e "$elapsed" --argjson nwh "$nodes_with_hot" \
                --argjson th "$total_hot" --arg ah "$avg_hot" \
                --argjson nc "$node_count" \
                '{elapsed_secs:$e, nodes_with_hot:$nwh, total_hot:$th, avg_hot:($ah|tonumber), node_count:$nc}' \
                >> "$samples_file" 2>/dev/null

            sleep "$interval"
        done
    ) &
    _REPORT_SAMPLER_PID=$!
    echo "  [report] Sampler started (PID ${_REPORT_SAMPLER_PID}, interval ${interval}s)"
}

# report_stop_sampler
report_stop_sampler() {
    _report_enabled || return 0
    if [ -n "$_REPORT_SAMPLER_PID" ]; then
        kill "$_REPORT_SAMPLER_PID" 2>/dev/null || true
        wait "$_REPORT_SAMPLER_PID" 2>/dev/null || true
        _REPORT_SAMPLER_PID=""
        echo "  [report] Sampler stopped"
    fi
}

# report_item_written ITEM_ID NODE_ID
report_item_written() {
    _report_enabled || return 0
    local item_id="$1" node_id="$2"
    local now
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local epoch
    epoch=$(date +%s)
    jq -n --arg id "$item_id" --arg n "$node_id" --arg t "$now" --argjson e "$epoch" \
        '{item_id:$id, written_on:$n, written_at:$t, epoch:$e}' \
        >> "$_REPORT_ITEMS_FILE"
}

# report_item_detected ITEM_ID NODE_ID
report_item_detected() {
    _report_enabled || return 0
    local item_id="$1" node_id="$2"
    local now
    now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local detect_epoch
    detect_epoch=$(date +%s)

    # Find write epoch for this item
    local write_epoch
    write_epoch=$(grep "\"item_id\":\"${item_id}\"" "$_REPORT_ITEMS_FILE" 2>/dev/null | head -1 | jq -r '.epoch' 2>/dev/null || echo "0")
    local latency=0
    if [ "$write_epoch" -gt 0 ]; then
        latency=$((detect_epoch - write_epoch))
    fi

    jq -n --arg id "$item_id" --arg n "$node_id" --arg t "$now" --argjson l "$latency" \
        '{item_id:$id, node:$n, detected_at:$t, latency_secs:$l}' \
        >> "$_REPORT_LATENCY_FILE"
}

# report_test LABEL STATUS [DETAIL]
report_test() {
    _report_enabled || return 0
    local label="$1" status="$2" detail="${3:-}"
    jq -n --arg n "$label" --arg s "$status" --arg d "$detail" \
        '{name:$n, status:$s, detail:$d}' \
        >> "$_REPORT_TESTS_FILE"
}

# report_finalize EXIT_CODE
report_finalize() {
    _report_enabled || return 0
    local exit_code="${1:-0}"
    local end_time
    end_time=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    local end_epoch
    end_epoch=$(date +%s)
    local duration=$(( end_epoch - _REPORT_START_EPOCH ))
    local status="PASSED"
    [ "$exit_code" -ne 0 ] && status="FAILED"

    # Stop sampler if running
    report_stop_sampler

    # Collect final snapshot
    echo "  [report] Collecting final snapshot..."
    _report_collect_diag "post"

    # --- Generate convergence.csv from samples.jsonl ---
    local csv_file="$REPORT_DIR/convergence.csv"
    echo "elapsed_secs,nodes_with_hot,total_hot,avg_hot" > "$csv_file"
    if [ -f "$REPORT_DIR/samples.jsonl" ]; then
        jq -r '[.elapsed_secs, .nodes_with_hot, .total_hot, .avg_hot] | @csv' \
            "$REPORT_DIR/samples.jsonl" >> "$csv_file" 2>/dev/null || true
    fi

    # --- Count orgs from ORG_SPEC ---
    local org_count=0
    local org_spec="${ORG_SPEC:-}"
    if [ -n "$org_spec" ]; then
        IFS=',' read -ra _oc <<< "$org_spec"
        org_count=${#_oc[@]}
    fi

    # --- Build final_state from post snapshot ---
    local post_snapshot="$REPORT_DIR/snapshots/post.json"
    local final_state='{}'
    if [ -f "$post_snapshot" ]; then
        final_state=$(jq '{
            healthy_nodes: [.[] | select(.error == null)] | length,
            total_nodes: length,
            total_hot: [.[] | (.peers.hot // 0)] | add,
            avg_hot: (([.[] | (.peers.hot // 0)] | add) / ([.[] | select(.error == null)] | length) * 100 | round / 100),
            total_items_synced: [.[] | (.replication.items_synced // 0)] | add,
            total_sync_errors: [.[] | (.replication.sync_errors // 0)] | add,
            nodes: [.[] | select(.error == null) | {
                entity_id: (.entity_id // .node),
                hot: (.peers.hot // 0),
                warm: (.peers.warm // 0),
                pushed: (.replication.items_pushed // 0),
                synced: (.replication.items_synced // 0),
                errors: (.replication.sync_errors // 0)
            }] | sort_by(-.synced)
        }' "$post_snapshot" 2>/dev/null || echo '{}')
    fi

    # --- Build phases array ---
    local phases='[]'
    if [ -s "$_REPORT_PHASES_FILE" ]; then
        phases=$(jq -s '.' "$_REPORT_PHASES_FILE" 2>/dev/null || echo '[]')
    fi

    # --- Build tests array ---
    local tests='[]'
    if [ -s "$_REPORT_TESTS_FILE" ]; then
        tests=$(jq -s '.' "$_REPORT_TESTS_FILE" 2>/dev/null || echo '[]')
    fi

    # --- Build replication items ---
    local replication='{"items":[]}'
    if [ -s "$_REPORT_ITEMS_FILE" ] && [ -s "$_REPORT_LATENCY_FILE" ]; then
        local items_json latency_json
        items_json=$(jq -s '.' "$_REPORT_ITEMS_FILE" 2>/dev/null || echo '[]')
        latency_json=$(jq -s '.' "$_REPORT_LATENCY_FILE" 2>/dev/null || echo '[]')
        replication=$(jq -n \
            --argjson w "$items_json" \
            --argjson d "$latency_json" \
            '{items: [$w[] | . as $item | {
                item_id: $item.item_id,
                written_on: $item.written_on,
                written_at: $item.written_at,
                replicas: [$d[] | select(.item_id == $item.item_id) | {
                    node: .node,
                    detected_at: .detected_at,
                    latency_secs: .latency_secs
                }]
            }]}' 2>/dev/null || echo '{"items":[]}')
    fi

    # --- Assemble metrics.json ---
    jq -n \
        --arg tn "$_REPORT_TEST_NAME" \
        --arg st "$status" \
        --arg si "$_REPORT_START" \
        --arg ei "$end_time" \
        --argjson d "$duration" \
        --argjson nc "$_REPORT_NODE_COUNT" \
        --argjson oc "$org_count" \
        --argjson phases "$phases" \
        --argjson tests "$tests" \
        --argjson repl "$replication" \
        --argjson fs "$final_state" \
        '{
            test_name: $tn,
            status: $st,
            start_time: $si,
            end_time: $ei,
            duration_secs: $d,
            topology: {node_count: $nc, org_count: $oc},
            phases: $phases,
            tests: $tests,
            replication: $repl,
            final_state: $fs
        }' > "$REPORT_DIR/metrics.json"

    # --- Generate graphs ---
    if [ -f "$REPORT_DIR/lib/graph.sh" ] || [ -f "/tests/lib/graph.sh" ]; then
        local graph_lib="${REPORT_DIR}/lib/graph.sh"
        [ -f "/tests/lib/graph.sh" ] && graph_lib="/tests/lib/graph.sh"
        . "$graph_lib"
        graph_convergence "$csv_file" "$REPORT_DIR/convergence.png"
        graph_latency "$REPORT_DIR/metrics.json" "$REPORT_DIR/latency.png"
    fi

    # --- Generate report.md ---
    _report_generate_markdown "$status" "$end_time" "$duration"

    # --- Clean up temp files ---
    rm -f "$_REPORT_PHASES_FILE" "$_REPORT_TESTS_FILE" "$_REPORT_ITEMS_FILE" "$_REPORT_LATENCY_FILE"

    echo ""
    echo "  [report] Report written to: ${REPORT_DIR}"
    echo "  [report]   report.md, metrics.json, convergence.csv"
    [ -f "$REPORT_DIR/convergence.png" ] && echo "  [report]   convergence.png"
    [ -f "$REPORT_DIR/latency.png" ] && echo "  [report]   latency.png"

    # Preserve original exit code through the trap
    return "$exit_code"
}

# --- Markdown report generation ---------------------------------------------

_report_generate_markdown() {
    local status="$1" end_time="$2" duration="$3"
    local md="$REPORT_DIR/report.md"

    # Count orgs
    local org_count=0
    local org_spec="${ORG_SPEC:-}"
    if [ -n "$org_spec" ]; then
        IFS=',' read -ra _oc <<< "$org_spec"
        org_count=${#_oc[@]}
    fi

    cat > "$md" <<EOF
# E2E Test Report: ${_REPORT_TEST_NAME}

**Status:** ${status} | **Date:** ${_REPORT_START} | **Duration:** ${duration}s
**Topology:** ${_REPORT_NODE_COUNT} nodes, ${org_count} orgs

## Results

EOF

    # Test results table
    if [ -f "$REPORT_DIR/metrics.json" ]; then
        echo "| # | Test | Status | Detail |" >> "$md"
        echo "|---|------|--------|--------|" >> "$md"
        jq -r '.tests | to_entries[] | "| \(.key + 1) | \(.value.name) | \(.value.status) | \(.value.detail) |"' \
            "$REPORT_DIR/metrics.json" >> "$md" 2>/dev/null || true
        echo "" >> "$md"
    fi

    # Phase timing table
    local has_phases
    has_phases=$(jq '.phases | length' "$REPORT_DIR/metrics.json" 2>/dev/null || echo 0)
    if [ "$has_phases" -gt 0 ]; then
        echo "## Phases" >> "$md"
        echo "" >> "$md"
        echo "| Phase | Status | Duration | Detail |" >> "$md"
        echo "|-------|--------|----------|--------|" >> "$md"
        jq -r '.phases[] | "| \(.name) | \(.status) | \(.duration_secs)s | \(.detail) |"' \
            "$REPORT_DIR/metrics.json" >> "$md" 2>/dev/null || true
        echo "" >> "$md"
    fi

    # Convergence section
    echo "## Convergence" >> "$md"
    echo "" >> "$md"

    # ASCII convergence graph
    if [ -f "$REPORT_DIR/.convergence_ascii.txt" ]; then
        echo '```' >> "$md"
        cat "$REPORT_DIR/.convergence_ascii.txt" >> "$md"
        echo '```' >> "$md"
        echo "" >> "$md"
        rm -f "$REPORT_DIR/.convergence_ascii.txt"
    fi

    # Convergence milestones from CSV
    if [ -f "$REPORT_DIR/convergence.csv" ]; then
        local total_nodes="$_REPORT_NODE_COUNT"
        if [ "$total_nodes" -gt 0 ]; then
            local t50 t95 t100
            t50=$(awk -F',' -v n="$total_nodes" 'NR>1 && $2 >= n*0.5 {print $1; exit}' "$REPORT_DIR/convergence.csv" || echo "n/a")
            t95=$(awk -F',' -v n="$total_nodes" 'NR>1 && $2 >= n*0.95 {print $1; exit}' "$REPORT_DIR/convergence.csv" || echo "n/a")
            t100=$(awk -F',' -v n="$total_nodes" 'NR>1 && $2 >= n {print $1; exit}' "$REPORT_DIR/convergence.csv" || echo "n/a")
            [ -z "$t50" ] && t50="n/a"
            [ -z "$t95" ] && t95="n/a"
            [ -z "$t100" ] && t100="n/a"
            echo "- Time to 50% converged: ${t50}s" >> "$md"
            echo "- Time to 95% converged: ${t95}s" >> "$md"
            echo "- Time to 100% converged: ${t100}s" >> "$md"
            echo "" >> "$md"
        fi
    fi

    [ -f "$REPORT_DIR/convergence.png" ] && echo "See \`convergence.png\` for full graph." >> "$md" && echo "" >> "$md"

    # Replication latency section
    local has_items
    has_items=$(jq '.replication.items | length' "$REPORT_DIR/metrics.json" 2>/dev/null || echo 0)
    if [ "$has_items" -gt 0 ]; then
        echo "## Replication Latency" >> "$md"
        echo "" >> "$md"
        echo "| Item | Written On | Replicas | Mean Latency |" >> "$md"
        echo "|------|-----------|----------|--------------|" >> "$md"
        jq -r '.replication.items[] |
            "| \(.item_id) | \(.written_on) | \(.replicas | length) | \(
                if (.replicas | length) > 0 then
                    "\(([.replicas[].latency_secs] | add) / ([.replicas[].latency_secs] | length))s"
                else "n/a" end
            ) |"' "$REPORT_DIR/metrics.json" >> "$md" 2>/dev/null || true
        echo "" >> "$md"

        [ -f "$REPORT_DIR/latency.png" ] && echo "See \`latency.png\` for histogram." >> "$md" && echo "" >> "$md"
    fi

    # Network state (final)
    echo "## Network State (Final)" >> "$md"
    echo "" >> "$md"

    if [ -f "$REPORT_DIR/metrics.json" ]; then
        local fs
        fs=$(jq '.final_state' "$REPORT_DIR/metrics.json" 2>/dev/null || echo '{}')
        if [ "$fs" != "{}" ] && [ "$fs" != "null" ]; then
            echo "| Metric | Value |" >> "$md"
            echo "|--------|-------|" >> "$md"
            jq -r '.final_state |
                "| Healthy nodes | \(.healthy_nodes // "?") / \(.total_nodes // "?") |\n| Total hot connections | \(.total_hot // "?") |\n| Avg hot peers | \(.avg_hot // "?") |\n| Items synced (total) | \(.total_items_synced // "?") |\n| Sync errors | \(.total_sync_errors // "?") |"' \
                "$REPORT_DIR/metrics.json" >> "$md" 2>/dev/null || true
            echo "" >> "$md"

            # Top 5 nodes by items synced
            local top5
            top5=$(jq '.final_state.nodes[:5] | length' "$REPORT_DIR/metrics.json" 2>/dev/null || echo 0)
            if [ "$top5" -gt 0 ]; then
                echo "## Top 5 Nodes by Items Synced" >> "$md"
                echo "" >> "$md"
                echo "| Node | Hot | Warm | Pushed | Synced | Errors |" >> "$md"
                echo "|------|-----|------|--------|--------|--------|" >> "$md"
                jq -r '.final_state.nodes[:5][] | "| \(.entity_id) | \(.hot) | \(.warm) | \(.pushed) | \(.synced) | \(.errors) |"' \
                    "$REPORT_DIR/metrics.json" >> "$md" 2>/dev/null || true
                echo "" >> "$md"
            fi
        fi
    fi
}
