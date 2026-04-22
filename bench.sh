#!/bin/sh
set -euo pipefail

RUNS=5
BIN=./target/release/dop
LEDGER="$1"

cargo build --release 2>&1

declare -A totals

run_bench() {
  local label="$1"
  shift
  local total=0
  echo -n "$label "
  for i in $(seq 1 $RUNS); do
    local t
    time -f "%e" -o /tmp/bench_time "$@" > /dev/null 2>/dev/null
    t=$(cat /tmp/bench_time)
    total=$(awk "BEGIN { printf \"%.3f\", $total + $t }")
    echo -n "."
  done
  local mean
  mean=$(awk "BEGIN { printf \"%.3f\", $total / $RUNS }")
  echo " ${mean}s (mean of $RUNS runs)"
  totals["$label"]=$mean
}

echo ""
echo "=== Benchmark: 1M transactions ==="
echo ""

run_bench "ledger accounts         " ledger -f $LEDGER accounts
run_bench "dop accounts .ledger    " "$BIN" accounts $LEDGER
run_bench "dop compile             " "$BIN" compile $LEDGER -o /tmp/out.dop
run_bench "dop accounts .dop       " "$BIN" accounts /tmp/out.dop

echo ""
echo "Done."
