#!/usr/bin/env bash
# Dungeon gate (COMPANIONS.md 14, PLAN.md 11.8 Phase 7): a lone player with three companions
# clears the tutorial dungeon.
#
#   scripts/check-dungeon.sh            the offline and the simulated-network acceptance tests;
#                                       then a real zone on loopback: one raid leader with
#                                       three recruits clears the gate and the Warden (the
#                                       clear, the drop, the leader's trial, the wire); then
#                                       the load: LEADERS leaders with their squads in one
#                                       zone, the tick and the minds measured against
#                                       budgets.toml [companions].
#   scripts/check-dungeon.sh --online   the whole path through the hub: three owners list
#                                       avatars in the tavern; the leader finds the keep
#                                       locked, clears the dungeon with recruits (the first
#                                       drops), hires the three avatars with the coin of that
#                                       kill, clears it again with them, and enters the keep.
#                                       Needs GM_TEST_DATABASE_URL (a Postgres this run wipes).
# Environment: LEADERS (default [companions].leaders), SKIP_BUILD=1, SKIP_TESTS=1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
LEADERS="${LEADERS:-$(budget companions leaders)}"
MAX_MIND_US="$(budget companions max_mind_us)"
MAX_NAV_MS="$(budget companions max_nav_build_ms)"
MAX_TICK_P99="$(budget companions max_tick_p99_us)"
MIN_CLEAR="$(budget companions min_clear_s)"
MAX_CLEAR="$(budget companions max_clear_s)"
MAX_BPS="$(budget companions max_bytes_per_player_s)"
MAP=assets/maps/built/dungeon.bsp
RECRUITS=ironclad,mender,frostweaver
DROP=core/iron,frame/ash,catalyst/basalt

mode=offline
for a in "$@"; do
  case "$a" in
    --online) mode=online ;;
    *) echo "usage: $0 [--online]"; exit 2 ;;
  esac
done
[[ -f "$MAP" ]] || { echo "check-dungeon: $MAP missing (gm-tools map gen-dungeon; gm-tools map build assets/maps/src/dungeon.map)"; exit 1; }
if [[ "${SKIP_BUILD:-}" != 1 ]]; then
  cargo build --release -p gm-server -p gm-bot --locked -q
  [[ $mode == online ]] && cargo build --release -p gm-hub --locked -q
fi

tmp="$(mktemp -d)"
pids=()
cleanup() { for p in "${pids[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"; }
trap cleanup EXIT
status=0
ok()   { echo "OK: $*"; }
fail() { echo "FAIL: $*"; status=1; }
atmost()  { if (( $(echo "$1 $2" | awk '{print ($1 > $2)}') )); then fail "$3 $1 exceeds $2"; else ok "$3 $1 within $2"; fi; }
atleast() { if (( $(echo "$1 $2" | awk '{print ($1 < $2)}') )); then fail "$3 $1 below $2"; else ok "$3 $1 at or above $2"; fi; }
equal()   { if [[ "$1" != "$2" ]]; then fail "$3 is $1, expected $2"; else ok "$3 is $1"; fi; }
# field LINE KEY: the value of `KEY=value` on a report line (to the next space).
field() { echo "$1" | sed -n "s/.*[ :]$2=\([^ ]*\).*/\1/p"; }
plain() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }

# What a raid leader's report line must say of a full clear. $1 = the line, $2 = hired.
raid_checks() {
  local line="$1" hired="$2" warden
  [[ -n "$line" ]] || { fail "the leader printed no raid report"; return; }
  echo "$line" | cut -c1-360
  equal "$(field "$line" done)" true "the raid finished"
  equal "$(field "$line" squad)" 3 "companions in the squad"
  equal "$(field "$line" hired)" "$hired" "of them hired"
  warden="$(field "$line" cleared | tr ',[]' '\n\n\n' | sed -n 's/^warden:\([0-9]*\)s$/\1/p')"
  [[ -n "$warden" ]] || { fail "the Warden was not cleared: $(field "$line" cleared)"; return; }
  atleast "$warden" "$MIN_CLEAR" "seconds the Warden took"
  atmost "$warden" "$MAX_CLEAR" "seconds the Warden took"
  equal "$(field "$line" loot)" "[$DROP]" "the drop"
  equal "$(field "$line" coin)" 30 "the coin"
  equal "$(field "$line" trials_passed)" "[warden_leader]" "trials passed"
  atmost "$(field "$line" rx_bytes_per_s)" "$MAX_BPS" "bytes/s down"
  # The netcode budget (PROTOCOL.md 9): fewer than one unexplained correction per 10 s.
  local secs; secs="$(field "$line" secs)"
  atmost "$(field "$line" unexplained)" $(( ${secs%.*} / 10 + 1 )) "unexplained corrections in ${secs%.*} s"
}

# The worst window of a zone log with $2 players: tick p99, microseconds per mind, overruns.
zone_checks() {
  local log="$1" players="$2" windows
  windows="$(plain "$log" | grep -E "zone report .* players=$players " || true)"
  [[ -n "$windows" ]] || { fail "no zone report with $players players"; return; }
  echo "$windows" | tail -1 | sed 's/^.*zone report/zone report/' | cut -c1-260
  local p99 per_mind overruns
  p99="$(echo "$windows" | sed -n 's/.* tick_us_p99=\([0-9.]*\).*/\1/p' | sort -g | tail -1)"
  per_mind="$(echo "$windows" | sed -n 's/.* minds=\([0-9]*\) minds_us=\([0-9]*\).*/\1 \2/p' | awk '$1 > 0 {v = $2 / $1; if (v > m) m = v} END {printf "%.1f", m}')"
  overruns="$(echo "$windows" | sed -n 's/.* overruns=\([0-9]*\).*/\1/p' | sort -g | tail -1)"
  atmost "$p99" "$MAX_TICK_P99" "zone tick p99 microseconds with $players players"
  atmost "$per_mind" "$MAX_MIND_US" "microseconds per mind per tick"
  equal "$overruns" 0 "zone tick overruns"
  atmost "$(plain "$log" | sed -n 's/.* nav_ms=\([0-9.]*\).*/\1/p' | head -1)" "$MAX_NAV_MS" "nav grid build, ms"
}

if [[ $mode == offline ]]; then
  if [[ "${SKIP_TESTS:-}" != 1 ]]; then
    cargo test --release --locked -q -p gm-ai --test dungeon --test skirmish -- --nocapture 2>&1 \
      | grep -E "^cleared on|^test result|FAILED|panicked" || true
    cargo test --release --locked -q -p gm-ai --test dungeon --test skirmish > /dev/null \
      && ok "offline: the dungeon and the open-ground fights" || fail "offline acceptance tests"
    cargo test --release --locked -q -p gm-server --test dungeon > /dev/null \
      && ok "simulated network (150 ms, 3% loss): the dungeon over the protocol" \
      || fail "the dungeon over the protocol"
  fi

  # One leader, three recruits, a real zone.
  port=$((20000 + RANDOM % 20000))
  target/release/gm-server --map "$MAP" --listen 127.0.0.1:$port --cert-out "$tmp/zone.der" \
    --squads --recruits "$RECRUITS" --report-secs 5 > "$tmp/zone.log" 2>&1 &
  zone=$!; pids+=("$zone")
  for _ in $(seq 1 50); do [[ -s "$tmp/zone.der" ]] && break; sleep 0.1; done
  target/release/gm-bot --connect 127.0.0.1:$port --cert "$tmp/zone.der" --map "$MAP" --bots 1 \
    --secs 420 --behaviour raid --builds blade > "$tmp/bot.log" 2>&1 \
    || { tail -5 "$tmp/bot.log"; echo "FAIL: the leader did not finish"; exit 1; }
  raid_checks "$(plain "$tmp/bot.log" | grep '^raid ' | tail -1)" 0
  zone_checks "$tmp/zone.log" 1
  kill "$zone" 2>/dev/null || true

  # The load: LEADERS leaders and their squads at once.
  port=$((port + 1))
  target/release/gm-server --map "$MAP" --listen 127.0.0.1:$port --cert-out "$tmp/load.der" \
    --squads --recruits "$RECRUITS" --report-secs 5 --max-players $((LEADERS + 4)) > "$tmp/load.log" 2>&1 &
  zone=$!; pids+=("$zone")
  for _ in $(seq 1 50); do [[ -s "$tmp/load.der" ]] && break; sleep 0.1; done
  target/release/gm-bot --connect 127.0.0.1:$port --cert "$tmp/load.der" --map "$MAP" --bots "$LEADERS" \
    --secs 60 --behaviour raid --builds blade > "$tmp/load-bots.log" 2>&1 \
    || { tail -5 "$tmp/load-bots.log"; echo "FAIL: the leaders did not finish"; exit 1; }
  sleep 6
  lines="$(plain "$tmp/load-bots.log" | grep '^raid ' || true)"
  plain "$tmp/load-bots.log" | grep -E '^bots=|^bytes/s' || true
  equal "$(echo "$lines" | grep -c ' squad=3 ')" "$LEADERS" "leaders with a full squad"
  # A leader that never saw a creature's health never got to the gate: it is stuck.
  equal "$(echo "$lines" | grep -vc ' creature_health_seen=0 ')" "$LEADERS" "leaders that reached the gate"
  worst_rx="$(echo "$lines" | sed -n 's/.* rx_bytes_per_s=\([0-9]*\).*/\1/p' | sort -g | tail -1)"
  atmost "$worst_rx" "$MAX_BPS" "bytes/s down of the worst leader among $LEADERS"
  zone_checks "$tmp/load.log" "$LEADERS"
  exit $status
fi

# --online
: "${GM_TEST_DATABASE_URL:?--online needs GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
PRICE=10
hub_port=$((20000 + RANDOM % 20000)); zone_port=$((hub_port + 1)); keep_port=$((hub_port + 2))
target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --wipe --listen 127.0.0.1:$hub_port \
  --cert-out "$tmp/hub.der" --key "$tmp/hub.key" --zone-secret "$tmp" --models-dir "$tmp/models" \
  --auth-per-minute 100000 > "$tmp/hub.log" 2>&1 &
pids+=("$!")
for _ in $(seq 1 50); do [[ -s "$tmp/hub.der" ]] && break; sleep 0.1; done
sleep 0.5
hub=(--hub 127.0.0.1:$hub_port --hub-cert "$tmp/hub.der" --maps-dir assets/maps/built)
start_dungeon() {
  target/release/gm-server --map "$MAP" --listen 127.0.0.1:$zone_port --cert-out "$tmp/zone.der" \
    --hub 127.0.0.1:$hub_port --hub-cert "$tmp/hub.der" --zone-id dungeon --zone-secret "$tmp" \
    --squads --recruits "$RECRUITS" --arrive-at-entry --report-secs 5 > "$tmp/zone-$1.log" 2>&1 &
  dungeon=$!; pids+=("$dungeon")
  sleep 1.5
}
start_dungeon 1
target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$keep_port \
  --cert-out "$tmp/keep.der" --hub 127.0.0.1:$hub_port --hub-cert "$tmp/hub.der" --zone-id keep \
  --zone-secret "$tmp" --requires warden_leader --report-secs 5 > "$tmp/keep.log" 2>&1 &
pids+=("$!")
sleep 1.5

# Three owners list an avatar each and stay offline.
target/release/gm-bot "${hub[@]}" --user 'owner-{i}@bots.test' --password owner-password --register \
  --character 'Avatar{i}' --builds "$RECRUITS" --bots 3 --zone dungeon --list-for-hire "$PRICE" \
  --secs 0 > "$tmp/owners.log" 2>&1 || { tail -5 "$tmp/owners.log"; echo "FAIL: the owners could not list"; exit 1; }
equal "$(plain "$tmp/owners.log" | sed -n 's/^hub bots=3 completed=\([0-9]*\).*/\1/p')" 3 "avatars listed in the tavern"

leader=("${hub[@]}" --user leader@bots.test --password leader-password --character Leader --builds blade)
# The keep is locked to a character without the trial.
if target/release/gm-bot "${leader[@]}" --register --zone keep --secs 2 --behaviour hold > "$tmp/keep-before.log" 2>&1; then
  fail "the keep let in a character without the leader's trial"
elif grep -q "locked: pass one of warden_leader" "$tmp/keep-before.log"; then
  ok "the keep is locked until the leader's trial is passed"
else
  tail -3 "$tmp/keep-before.log"; fail "the keep refused for another reason"
fi

# Run 1: with the zone's recruits. The first drops and the first coin.
target/release/gm-bot "${leader[@]}" --zone dungeon --secs 420 --behaviour raid > "$tmp/run1.log" 2>&1 \
  || { tail -5 "$tmp/run1.log"; echo "FAIL: the first run did not finish"; exit 1; }
raid_checks "$(plain "$tmp/run1.log" | grep '^raid ' | tail -1)" 0
after1="$(plain "$tmp/run1.log" | grep '^character ' | tail -1)"
echo "$after1"
equal "$(field "$after1" coin)" 30 "silver in the inventory after the first kill"
equal "$(field "$after1" items)" "[$DROP]" "components in the inventory"
equal "$(field "$after1" trials)" "[warden_leader]" "trials on the character"
zone_checks "$tmp/zone-1.log" 1

# Run 2: a fresh dungeon; the three avatars hired with the coin of that kill.
kill "$dungeon" 2>/dev/null || true; sleep 1
start_dungeon 2
target/release/gm-bot "${leader[@]}" --zone dungeon --secs 420 --behaviour raid --hire 3 > "$tmp/run2.log" 2>&1 \
  || { tail -5 "$tmp/run2.log"; echo "FAIL: the second run did not finish"; exit 1; }
raid_checks "$(plain "$tmp/run2.log" | grep '^raid ' | tail -1)" 3
after2="$(plain "$tmp/run2.log" | grep '^character ' | tail -1)"
echo "$after2"
equal "$(field "$after2" squad)" "[Avatar000,Avatar001,Avatar002]" "the squad, by name"
equal "$(field "$after2" coin)" 30 "silver after three hires of $PRICE and a second kill"
equal "$(echo "$(field "$after2" items)" | tr ',' '\n' | wc -l)" 6 "components after two kills"

# The trial opens the keep.
if target/release/gm-bot "${leader[@]}" --zone keep --secs 2 --behaviour hold > "$tmp/keep-after.log" 2>&1; then
  ok "the keep opens to a character that passed the leader's trial"
else
  tail -3 "$tmp/keep-after.log"; fail "the keep stayed locked"
fi
exit $status
