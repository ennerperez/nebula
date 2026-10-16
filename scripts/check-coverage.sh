#!/usr/bin/env bash
set -euo pipefail

threshold="${COVERAGE_THRESHOLD:-85}"
report="$(mktemp)"
trap 'rm -f "$report"' EXIT

cargo +nightly llvm-cov \
  --locked \
  --bin nebular \
  --branch \
  --fail-under-lines "$threshold" \
  | tee "$report"

branch_coverage="$({
  awk '$1 == "TOTAL" { value = $NF; gsub(/%/, "", value); print value }' "$report"
} | tail -n 1)"

if [[ -z "$branch_coverage" ]]; then
  echo "error: branch coverage total missing from cargo-llvm-cov report" >&2
  exit 1
fi

if ! awk -v actual="$branch_coverage" -v minimum="$threshold" \
  'BEGIN { exit !(actual + 0 >= minimum + 0) }'; then
  echo "error: branch coverage ${branch_coverage}% is below ${threshold}%" >&2
  exit 1
fi

echo "coverage gate passed: lines >= ${threshold}%, branches ${branch_coverage}%"

