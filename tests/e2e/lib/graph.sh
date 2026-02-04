#!/usr/bin/env bash
# Graph generation for E2E test reports.
# Uses gnuplot for PNG charts and generates ASCII fallbacks.
#
# Usage:
#   . lib/graph.sh
#   graph_convergence convergence.csv convergence.png
#   graph_latency metrics.json latency.png

# graph_convergence CSV_FILE PNG_FILE
# Dual Y-axis: nodes_with_hot (left), avg_hot (right).
# Also generates ASCII version at .convergence_ascii.txt in report dir.
graph_convergence() {
    local csv_file="$1" png_file="$2"
    local report_dir
    report_dir=$(dirname "$png_file")

    # Skip if no data or only header
    local lines
    lines=$(wc -l < "$csv_file" 2>/dev/null || echo 0)
    if [ "$lines" -le 1 ]; then
        echo "  [graph] No convergence data to plot"
        return 0
    fi

    # Generate ASCII convergence chart
    _graph_ascii_convergence "$csv_file" "$report_dir/.convergence_ascii.txt"

    # Generate PNG with gnuplot
    if command -v gnuplot > /dev/null 2>&1; then
        gnuplot <<GNUPLOT
set terminal png size 1200,600 font "Arial,12"
set output "${png_file}"
set title "Network Convergence"
set xlabel "Time (seconds)"
set ylabel "Nodes with Hot Peers"
set y2label "Avg Hot Peers"
set y2tics
set ytics nomirror
set grid
set key top left
set datafile separator ","

plot "${csv_file}" using 1:2 skip 1 with linespoints title "Nodes Converged" axes x1y1 lw 2 pt 7 ps 0.5, \
     "${csv_file}" using 1:4 skip 1 with lines title "Avg Hot Peers" axes x1y2 lw 2 dt 2
GNUPLOT
        echo "  [graph] Convergence PNG: ${png_file}"
    else
        echo "  [graph] gnuplot not available, skipping PNG"
    fi
}

# graph_latency METRICS_JSON PNG_FILE
# Histogram of replication latencies from item tracking data.
graph_latency() {
    local metrics_file="$1" png_file="$2"

    # Check if there are any replication items
    local item_count
    item_count=$(jq '.replication.items | length' "$metrics_file" 2>/dev/null || echo 0)
    if [ "$item_count" -eq 0 ]; then
        return 0
    fi

    # Extract latencies to temp file
    local tmp_lat
    tmp_lat=$(mktemp)
    jq -r '.replication.items[].replicas[].latency_secs' "$metrics_file" > "$tmp_lat" 2>/dev/null

    local lat_count
    lat_count=$(wc -l < "$tmp_lat" 2>/dev/null || echo 0)
    if [ "$lat_count" -eq 0 ]; then
        rm -f "$tmp_lat"
        return 0
    fi

    if command -v gnuplot > /dev/null 2>&1; then
        # Find max latency for binning
        local max_lat
        max_lat=$(sort -n "$tmp_lat" | tail -1)
        local bin_width
        bin_width=$(echo "scale=0; ($max_lat + 9) / 10" | bc 2>/dev/null || echo 5)
        [ "$bin_width" -lt 1 ] && bin_width=1

        gnuplot <<GNUPLOT
set terminal png size 800,400 font "Arial,12"
set output "${png_file}"
set title "Replication Latency Distribution"
set xlabel "Latency (seconds)"
set ylabel "Count"
set style fill solid 0.7
set boxwidth ${bin_width} * 0.9
bin_width = ${bin_width}
bin(x) = bin_width * floor(x / bin_width)
set grid ytics

plot "${tmp_lat}" using (bin(\$1)):(1.0) smooth freq with boxes title "Latency" lc rgb "#4472C4"
GNUPLOT
        echo "  [graph] Latency PNG: ${png_file}"
    fi

    rm -f "$tmp_lat"
}

# --- ASCII chart helpers -----------------------------------------------------

_graph_ascii_convergence() {
    local csv_file="$1" out_file="$2"
    local width=60
    local height=20

    # Read data (skip header)
    local data
    data=$(tail -n +2 "$csv_file" 2>/dev/null)
    [ -z "$data" ] && return 0

    # Find max values for scaling
    local max_nodes max_time
    max_nodes=$(echo "$data" | awk -F',' '{print $2}' | sort -n | tail -1)
    max_time=$(echo "$data" | awk -F',' '{print $1}' | sort -n | tail -1)
    [ "$max_nodes" -eq 0 ] && max_nodes=1
    [ "$max_time" -eq 0 ] && max_time=1

    {
        echo "Nodes Converged (${max_nodes} max)"
        echo "  |"

        # Sample data points to fit width
        local total_lines
        total_lines=$(echo "$data" | wc -l)
        local step=1
        [ "$total_lines" -gt "$width" ] && step=$(( total_lines / width ))

        # Build simple bar chart (vertical, sampled)
        local i=0
        for row_num in $(seq 1 "$step" "$total_lines"); do
            local row
            row=$(echo "$data" | sed -n "${row_num}p")
            local t n
            t=$(echo "$row" | awk -F',' '{print $1}')
            n=$(echo "$row" | awk -F',' '{print $2}')
            local bar_len=$(( n * width / max_nodes ))
            local bar=""
            for b in $(seq 1 "$bar_len"); do bar="${bar}#"; done
            printf "%3ss |%-${width}s| %s/%s\n" "$t" "$bar" "$n" "$max_nodes"
        done

        echo "  +$(printf '%0.s-' $(seq 1 $((width + 1))))"
        echo "    Time (seconds) -> ${max_time}s"
    } > "$out_file"
}
