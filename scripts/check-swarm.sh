#!/usr/bin/env bash
# Swarm gate (HUB.md 5, PLAN.md 11.8 Phase 4): N duelist bots on one zone over real UDP on
# loopback, everyone in the arena hall (nothing culled, the worst case). The server's tick time,
# RSS and per-player bytes must stay under budgets.toml [server] and [net].
#
#   scripts/check-swarm.sh            200 bots for 20 s (the reference gate)
#   BOTS=32 scripts/check-swarm.sh    a smaller smoke run (CI)
# Environment: BOTS (default 200), SECS (default 20), SKIP_BUILD=1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BOTS="${BOTS:-200}"
SECS="${SECS:-20}"
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
MAX_MEAN="$(budget server max_tick_mean_us)"
MAX_P99="$(budget server max_tick_p99_us)"
MAX_RSS="$(budget server max_rss_bytes)"
MAX_BPS="$(budget net max_bytes_per_player_s)"
[[ -f assets/maps/built/arena.bsp ]] || { echo "check-swarm: assets/maps/built/arena.bsp missing"; exit 1; }
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-server -p gm-bot --locked -q

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"; kill $SERVER_PID 2>/dev/null || true' EXIT
port=$((20000 + RANDOM % 20000))
ticks=$(( (SECS + 8) * 64 ))
target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$port --cert-out "$tmp/cert.der" \
  --max-players $((BOTS + 8)) --ticks $ticks --report-secs 5 > "$tmp/server.log" 2>&1 &
SERVER_PID=$!
sleep 1.5
target/release/gm-bot --connect 127.0.0.1:$port --cert "$tmp/cert.der" --map assets/maps/built/arena.bsp \
  --bots "$BOTS" --secs "$SECS" --behaviour duelist --builds ironclad,blade,frostweaver,shade --teams 1,2 > "$tmp/bots.log" 2>&1 &
BOTS_PID=$!
# Sample the server's peak RSS while the swarm is in.
rss=0
for _ in $(seq 1 $((SECS + 6))); do
  sleep 1
  cur="$(awk '/VmHWM/{print $2*1024}' /proc/$SERVER_PID/status 2>/dev/null || echo 0)"
  (( cur > rss )) && rss=$cur || true
done
wait $BOTS_PID || { cat "$tmp/bots.log"; echo "FAIL: bots did not finish"; exit 1; }
wait $SERVER_PID || true
sed 's/\x1b\[[0-9;]*m//g' "$tmp/server.log" | grep -E 'zone report|final report' | tail -4
tail -4 "$tmp/bots.log"
# The last full window with every bot present: the report line whose players == BOTS.
line="$(sed 's/\x1b\[[0-9;]*m//g' "$tmp/server.log" | grep -E "zone report tick=[0-9]+ players=$BOTS " | tail -1)"
[[ -n "$line" ]] || { echo "FAIL: no report window with $BOTS players"; exit 1; }
field() { echo "$line" | sed -n "s/.* $1=\([0-9.]*\).*/\1/p"; }
mean="$(field tick_us_mean)"; p99="$(field tick_us_p99)"; tx="$(field tx_bps)"
status=0
check() { # value max label
  local v="${1%.*}" m="$2" l="$3"
  if (( v > m )); then echo "FAIL: $l $v exceeds $m"; status=1; else echo "OK: $l $v within $m"; fi
}
check "$mean" "$MAX_MEAN" "tick mean us ($BOTS bots)"
check "$p99" "$MAX_P99" "tick p99 us ($BOTS bots)"
check "$rss" "$MAX_RSS" "server peak RSS bytes"
check "$tx" "$MAX_BPS" "server tx bytes/player/s"
exit $status
