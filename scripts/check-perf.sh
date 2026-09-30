#!/usr/bin/env bash
# Runtime gate: render FRAMES frames of the test map with the release client, then check the
# client's own peak-RSS report against budgets.toml [client].max_rss_bytes.
#
#   scripts/check-perf.sh                windowed on the current display, real GPU, vsync off
#   scripts/check-perf.sh --software     headless under a software Vulkan adapter (CI smoke test)
#   scripts/check-perf.sh --gate-fps     also fail below [client].min_fps (self-hosted iGPU runner)
# Environment: FRAMES (default 600), MAP, SKIP_BUILD=1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
FRAMES="${FRAMES:-600}"
MAP="${MAP:-assets/maps/built/test_room.bsp}"
budget() { awk -v key="$1" '/^\[/{sec=$1} sec=="[client]" && $1==key {print $3; exit}' budgets.toml; }
MAX_RSS="$(budget max_rss_bytes)"
MIN_FPS="$(budget min_fps)"

args=(--map "$MAP" --bench "$FRAMES" --no-vsync)
gate_fps=0
for a in "$@"; do
  case "$a" in
    --software) args+=(--headless --software) ;;
    --headless) args+=(--headless) ;;
    --gate-fps) gate_fps=1 ;;
    *) echo "usage: $0 [--software|--headless] [--gate-fps]"; exit 2 ;;
  esac
done

[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-client --locked -q
out="$(target/release/gm-client "${args[@]}" 2>&1)" || { echo "$out"; echo "FAIL: gm-client exited with an error"; exit 1; }
echo "$out"

rss="$(echo "$out" | sed -n 's/^bench: peak_rss_bytes=\([0-9]*\).*/\1/p' | tail -1)"
fps="$(echo "$out" | sed -n 's/^bench: fps_avg=\([0-9]*\)\..*/\1/p' | tail -1)"
[[ -n "$rss" ]] || { echo "FAIL: gm-client did not report peak RSS"; exit 1; }
status=0
if (( rss > MAX_RSS )); then echo "FAIL: peak RSS $rss bytes exceeds $MAX_RSS"; status=1; else echo "OK: peak RSS $rss bytes within $MAX_RSS"; fi
if (( gate_fps )); then
  [[ -n "$fps" ]] || { echo "FAIL: gm-client did not report fps"; exit 1; }
  if (( fps < MIN_FPS )); then echo "FAIL: average fps $fps below $MIN_FPS"; status=1; else echo "OK: average fps $fps at or above $MIN_FPS"; fi
fi
exit $status
