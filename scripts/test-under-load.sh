#!/usr/bin/env bash
set -eu

if [[ $# != 2 || ! $1 =~ ^[1-9][0-9]*$ || ! $2 =~ ^(0|[1-9][0-9]*)$ ]]; then
    echo "Usage: $0 RUNS LOAD_WORKERS" >&2
    exit 2
fi

cd "$(dirname "$0")/.."
runs=$1
workers=$2
mkdir -p target
logs=$(mktemp -d target/test-under-load.XXXXXX)
pids=()

cleanup() {
    for pid in "${pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

for ((worker = 0; worker < workers; worker++)); do
    (while :; do :; done) &
    pids+=("$!")
done

export PEPINO_NETWORK_TESTS=1
unset RUST_TEST_THREADS
echo "Running $runs workspace suites with $workers load workers. Logs: $logs"
failed=0
SECONDS=0
for ((run = 1; run <= runs; run++)); do
    started=$SECONDS
    log="$logs/run-$run.log"
    status=0
    cargo nextest run --workspace --profile default --no-fail-fast >"$log" 2>&1 || status=$?
    cargo test --workspace --doc >>"$log" 2>&1 || status=$?
    if ((status == 0)); then
        echo "Run $run/$runs passed in $((SECONDS - started))s"
    else
        echo "Run $run/$runs failed in $((SECONDS - started))s"
        cat "$log"
        failed=$((failed + 1))
    fi
done
echo "$((runs - failed))/$runs passed, $failed failed, ${SECONDS}s total"
((failed == 0))
