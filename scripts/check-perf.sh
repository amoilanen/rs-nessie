#!/usr/bin/env bash
# scripts/check-perf.sh
#
# Run the `nessie-core::frame` Criterion bench against the saved `ci`
# baseline and fail (exit non-zero) if mean throughput regresses by more
# than 10%.
#
# Usage:
#   scripts/check-perf.sh             # default: 10% regression threshold
#   THRESHOLD_PCT=15 scripts/check-perf.sh
#
# Expected baseline:
#   The script assumes a baseline named `ci` already exists for the
#   `step_frame` benchmark. Generate one with:
#
#       cargo bench -p nessie-core --bench frame -- --save-baseline ci
#
#   The baseline lives under `target/criterion/<bench>/ci/`.
#
# CI wiring:
#   This script is wired into the **nightly** workflow only — per-PR runs
#   on shared GitHub runners are too noisy for a hard perf gate (spec
#   §8.3 calls this out explicitly).
#
# Implementation notes:
#   Criterion's `--baseline <name>` mode re-runs the bench and writes the
#   comparison to `change/estimates.json`. We parse the `mean.point_estimate`
#   from that file (a fractional change, e.g. 0.07 = +7%) and compare it
#   against the configured threshold. A positive value means the bench got
#   *slower* (regression). We deliberately do not fail on improvements.

set -euo pipefail

THRESHOLD_PCT="${THRESHOLD_PCT:-10}"
BENCH_NAME="step_frame"
BASELINE_NAME="ci"
CRITERION_DIR="target/criterion/${BENCH_NAME}"
CHANGE_FILE="${CRITERION_DIR}/change/estimates.json"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

if [[ ! -d "${CRITERION_DIR}/${BASELINE_NAME}" ]]; then
  echo "error: no Criterion baseline '${BASELINE_NAME}' found at ${CRITERION_DIR}/${BASELINE_NAME}" >&2
  echo "       generate one first with:" >&2
  echo "         cargo bench -p nessie-core --bench frame -- --save-baseline ${BASELINE_NAME}" >&2
  exit 2
fi

echo "==> Running bench against baseline '${BASELINE_NAME}' (regression threshold: ${THRESHOLD_PCT}%)"
# --noplot avoids the gnuplot/plotters back-end on headless CI runners.
cargo bench -p nessie-core --bench frame -- --baseline "${BASELINE_NAME}" --noplot

if [[ ! -f "${CHANGE_FILE}" ]]; then
  echo "error: Criterion did not produce ${CHANGE_FILE}; cannot evaluate regression." >&2
  exit 3
fi

# Extract `mean.point_estimate` (fractional change vs baseline). Prefer
# `jq` when available; fall back to a tiny Python one-liner so the script
# works in CI containers without `jq` installed.
if command -v jq >/dev/null 2>&1; then
  change_frac="$(jq -r '.mean.point_estimate' "${CHANGE_FILE}")"
elif command -v python3 >/dev/null 2>&1; then
  change_frac="$(python3 -c "import json,sys;print(json.load(open('${CHANGE_FILE}'))['mean']['point_estimate'])")"
else
  echo "error: need either 'jq' or 'python3' to parse ${CHANGE_FILE}" >&2
  exit 4
fi

# Convert fractional change → percent with 2 decimals. AWK is portable and
# avoids relying on bash's lack of float arithmetic.
change_pct="$(awk -v v="${change_frac}" 'BEGIN { printf "%.2f", v * 100 }')"
echo "==> Bench mean change vs baseline '${BASELINE_NAME}': ${change_pct}%"

# Regression iff positive change exceeds the threshold.
is_regression="$(awk -v c="${change_pct}" -v t="${THRESHOLD_PCT}" 'BEGIN { print (c > t) ? 1 : 0 }')"
if [[ "${is_regression}" == "1" ]]; then
  echo "FAIL: ${BENCH_NAME} regressed by ${change_pct}% (threshold: ${THRESHOLD_PCT}%)" >&2
  exit 1
fi

echo "OK: ${BENCH_NAME} within ${THRESHOLD_PCT}% of baseline."
