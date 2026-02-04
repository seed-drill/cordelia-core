#!/usr/bin/env bash
# E2e test: all N nodes reach hot >= 1, zero sync errors.
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
. "$DIR/lib/api.sh"
. "$DIR/lib/wait.sh"

N="${N:-3}"
TIMEOUT="${TIMEOUT:-180}"

# --- Report init (opt-in) ---------------------------------------------------

if [ "${REPORT:-0}" = "1" ]; then
    . "$DIR/lib/report.sh"
    report_init "convergence" "$N"
    report_snapshot "pre"
    report_start_sampler 10
    trap 'report_finalize $?' EXIT
fi

echo "=== Convergence test: ${N} nodes ==="

# Wait for all nodes healthy
[ "${REPORT:-0}" = "1" ] && report_phase_start "wait_all_healthy"
if wait_all_healthy "$N" "$TIMEOUT"; then
    [ "${REPORT:-0}" = "1" ] && report_test "wait_all_healthy" "PASS" "${N}/${N} healthy"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "wait_all_healthy" "PASS" "${N}/${N} healthy"
else
    [ "${REPORT:-0}" = "1" ] && report_test "wait_all_healthy" "FAIL" "timeout after ${TIMEOUT}s"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "wait_all_healthy" "FAIL" "timeout"
    exit 1
fi

# Wait for all nodes to have at least 1 hot peer
[ "${REPORT:-0}" = "1" ] && report_phase_start "wait_all_hot"
if wait_all_hot "$N" 1 "$TIMEOUT"; then
    [ "${REPORT:-0}" = "1" ] && report_test "wait_all_hot" "PASS" "All nodes >= 1 hot"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "wait_all_hot" "PASS" "All nodes >= 1 hot"
else
    [ "${REPORT:-0}" = "1" ] && report_test "wait_all_hot" "FAIL" "timeout after ${TIMEOUT}s"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "wait_all_hot" "FAIL" "timeout"
    exit 1
fi

# Check zero sync errors
[ "${REPORT:-0}" = "1" ] && report_phase_start "check_zero_sync_errors"
if check_zero_sync_errors "$N"; then
    [ "${REPORT:-0}" = "1" ] && report_test "check_zero_sync_errors" "PASS" "0 errors"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "check_zero_sync_errors" "PASS" "0 errors"
else
    [ "${REPORT:-0}" = "1" ] && report_test "check_zero_sync_errors" "FAIL" "sync errors found"
    [ "${REPORT:-0}" = "1" ] && report_phase_end "check_zero_sync_errors" "FAIL" "sync errors"
    exit 1
fi

echo "=== Convergence test PASSED ==="
