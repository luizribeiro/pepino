#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
export LC_ALL=C PEPINO_NETWORK_TESTS=1
budget=${PEPINO_SUITE_BUDGET_SECONDS:-120}
if [[ ! $budget =~ ^[1-9][0-9]*$ ]]; then
    echo 'PEPINO_SUITE_BUDGET_SECONDS must be a positive integer.' >&2
    exit 2
fi

load_average() {
    uptime | awk '/load averages?:/ {
        sub(/^.*load averages?:[[:space:]]*/, "")
        sub(/,.*/, "")
        print $1
    }'
}

cpu_count() {
    if command -v nproc >/dev/null; then
        nproc
    else
        getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu
    fi
}

cpus=$(cpu_count)
load=$(load_average)
log=$(mktemp)
trap 'rm -f "$log"' EXIT

echo "Running the network-enabled workspace suite. Budget: ${budget}s."
SECONDS=0
status=0
cargo nextest run --workspace --profile default --no-fail-fast --status-level pass \
    --message-format human --color never --show-progress none >"$log" 2>&1 || status=$?
if ((status != 0)); then
    cat "$log"
fi
cargo test --workspace --doc || status=$?
elapsed=$SECONDS

printf 'Workspace wall time: %ss\nSlowest ten nextest tests:\n' "$elapsed"
awk -F '[][]' '$1 ~ /^[[:space:]]*(PASS|FAIL|LEAK|TIMEOUT)[[:space:]]+$/ && $2 ~ /s$/ {
    sub(/^[[:space:]]+/, "", $2)
    sub(/^[[:space:]]+/, "", $3)
    print $2, $3
}' "$log" | sort -nr | sed -n '1,10p'

if ((status != 0)); then
    exit "$status"
fi
if [[ ! $load =~ ^[0-9]+(\.[0-9]+)?$ ]]; then
    echo 'Suite budget enforcement skipped because the one-minute load was unavailable.'
elif awk -v load_avg="$load" -v cpus="$cpus" 'BEGIN { exit !(load_avg >= 0.75 * cpus) }'; then
    echo "Suite budget enforcement skipped due to host load at launch ($load across $cpus CPUs)."
elif awk -v elapsed="$elapsed" -v budget="$budget" 'BEGIN { exit !(elapsed > budget) }'; then
    echo "Suite exceeded its ${budget}s budget." >&2
    exit 1
fi
