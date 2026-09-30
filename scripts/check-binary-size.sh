#!/usr/bin/env bash
# Binary-size gate for gm-client (PLAN.md 2.7 and 11.8, Phase 0/1).
# Fails when the release binary exceeds budgets.toml [client].max_binary_bytes, or when it grows
# by more than max_regression_bytes over ci/baselines/gm-client-size.
#
#   scripts/check-binary-size.sh                     build release gm-client and check it
#   scripts/check-binary-size.sh --update-baseline   record the current size as the baseline
#   scripts/check-binary-size.sh --self-test         prove that a regression and a cap breach fail
#   BINARY=path SKIP_BUILD=1 scripts/check-binary-size.sh     check an existing file instead
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BUDGETS="${BUDGETS_FILE:-$ROOT/budgets.toml}"
BASELINE_FILE="${BASELINE_FILE:-$ROOT/ci/baselines/gm-client-size}"
BINARY="${BINARY:-$ROOT/target/release/gm-client}"

budget() { awk -v key="$1" '/^\[/{sec=$1} sec=="[client]" && $1==key {print $3; exit}' "$BUDGETS"; }
MAX_BYTES="$(budget max_binary_bytes)"
MAX_REGRESSION="$(budget max_regression_bytes)"
[[ -n "$MAX_BYTES" && -n "$MAX_REGRESSION" ]] || { echo "check-binary-size: [client] keys missing in $BUDGETS"; exit 2; }

file_size() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; }
human() { awk -v b="$1" 'BEGIN { printf "%.2f MiB", b / 1048576 }'; }
build() { [[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-client --locked -q; }

check() { # <binary> <baseline-file>; prints the verdict, returns 1 on failure
  local bin="$1" base_file="$2" size status=0
  size="$(file_size "$bin")"
  echo "gm-client: $(human "$size") ($size bytes); absolute cap $(human "$MAX_BYTES")"
  if (( size > MAX_BYTES )); then echo "FAIL: exceeds the absolute cap"; status=1; fi
  if [[ -f "$base_file" ]]; then
    local base delta
    base="$(tr -dc '0-9' < "$base_file")"
    delta=$(( size - base ))
    echo "baseline: $(human "$base"); delta: $delta bytes; allowed regression: $MAX_REGRESSION bytes"
    if (( delta > MAX_REGRESSION )); then echo "FAIL: regression of $delta bytes exceeds $MAX_REGRESSION"; status=1; fi
  else
    echo "no baseline at $base_file; only the absolute cap applies"
  fi
  return $status
}

case "${1:-}" in
  --self-test)
    build
    tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
    file_size "$BINARY" > "$tmp/base"
    check "$BINARY" "$tmp/base" >/dev/null || { echo "self-test FAIL: the real binary was rejected against its own baseline"; exit 1; }
    cp "$BINARY" "$tmp/regressed"; truncate -s "+$((MAX_REGRESSION + 1))" "$tmp/regressed"
    if check "$tmp/regressed" "$tmp/base" >/dev/null; then echo "self-test FAIL: a $((MAX_REGRESSION + 1)) byte regression was accepted"; exit 1; fi
    truncate -s "$((MAX_BYTES + 1))" "$tmp/huge"
    if check "$tmp/huge" "$tmp/no-baseline" >/dev/null; then echo "self-test FAIL: a cap breach was accepted"; exit 1; fi
    echo "self-test OK: a $((MAX_REGRESSION + 1)) byte regression and a $((MAX_BYTES + 1)) byte binary both fail the gate"
    ;;
  --update-baseline)
    build
    mkdir -p "$(dirname "$BASELINE_FILE")"
    file_size "$BINARY" > "$BASELINE_FILE"
    echo "baseline updated: $(cat "$BASELINE_FILE") bytes -> ${BASELINE_FILE#"$ROOT"/}"
    ;;
  "")
    build
    check "$BINARY" "$BASELINE_FILE"
    ;;
  *) echo "usage: $0 [--self-test | --update-baseline]"; exit 2 ;;
esac
