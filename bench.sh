#!/usr/bin/env bash
set -euo pipefail

RUNS=5
BIN=./target/release/ledger

cargo build --release 2>&1

declare -A totals

run_bench() {
  local label="$1"
  shift
  local total=0
  echo -n "$label "
  for i in $(seq 1 $RUNS); do
    local t
    /run/current-system/sw/bin/time -f "%e" -o /tmp/bench_time "$@" > /dev/null 2>/dev/null
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

run_bench "ledger balance         " ledger -f benchmark.ledger balance
run_bench "ledgerrs balance .ledger" "$BIN" balance benchmark.ledger
run_bench "ledgerrs compile       " "$BIN" compile benchmark.ledger -o /tmp/out.bki
run_bench "ledgerrs balance .bki  " "$BIN" balance /tmp/out.bki

echo ""
echo "Done."
