#!/usr/bin/env bash
# Anti-cheat gate (docs/ANTICHEAT.md 10, PLAN.md 11.8 Phase 9): aim statistics, replays,
# reports, reputation.
#
#   scripts/check-anticheat.sh            synthetic view traces produce exactly their flags;
#                                         then a recorded arena with bots that aim like a hand
#                                         and bots that aim by program: every cheat is flagged
#                                         and no hand is, the fight's replay reads back, its
#                                         statistics recompute to what the zone said, and the
#                                         client renders it from a cheat's eyes
#   scripts/check-anticheat.sh --online   also through the hub (needs GM_TEST_DATABASE_URL, a
#                                         Postgres this run wipes): statistics and replays in
#                                         the database, a report upheld, a ban, a trust-gated
#                                         zone, team kills in the ledger
#   scripts/check-anticheat.sh --swarm    also the swarm gate with the recorder on (200 bots)
# Environment: SECS, SKIP_BUILD=1, KEEP=DIR (keep logs, replays and the screenshot there).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ONLINE=0; SWARM=0
for a in "$@"; do
  case "$a" in
    --online) ONLINE=1 ;;
    --swarm) SWARM=1 ;;
    *) echo "check-anticheat: unknown argument $a"; exit 2 ;;
  esac
done
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
HONEST="$(budget anticheat honest)"; CHEATS="$(budget anticheat cheats)"
SECS="${SECS:-$(budget anticheat secs)}"
status=0
ok()   { echo "OK: $*"; }
fail() { echo "FAIL: $*"; status=1; }
number() { [[ "$1" =~ ^[0-9]+(\.[0-9]+)?$ ]]; }
atmost() { # value max label
  if ! number "$1" || ! number "$2"; then fail "$3: no number to check ('$1' against '$2')"
  elif (( ${1%.*} > $2 )); then fail "$3 $1 exceeds $2"; else ok "$3 $1 within $2"; fi
}
atleast() {
  if ! number "$1" || ! number "$2"; then fail "$3: no number to check ('$1' against '$2')"
  elif (( ${1%.*} < $2 )); then fail "$3 $1 below $2"; else ok "$3 $1 at least $2"; fi
}

# 1. The analysis on traces whose answer is known.
out="$(cargo test -q -p gm-replay --test aim 2>&1)" || { echo "$out" | tail -20; echo "FAIL: the aim analysis tests"; exit 1; }
passed="$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | sort -n | tail -1)"
atleast "${passed:-0}" 10 "synthetic traces that produce exactly their flags"
# The zone's live numbers are the file's, and a long fight is cut into files that read.
out="$(cargo test -q -p gm-server --lib recorder 2>&1)" || { echo "$out" | tail -20; echo "FAIL: the recorder tests"; exit 1; }
passed="$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | sort -n | tail -1)"
atleast "${passed:-0}" 2 "recorder tests (live numbers equal the file's; the two-minute cut)"

# 2. A recorded arena.
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-server -p gm-bot -p gm-tools -p gm-client --locked -q
tmp="${KEEP:-$(mktemp -d)}"; mkdir -p "$tmp"; rm -rf "$tmp/replays"
SERVER_PID=
cleanup() { kill $SERVER_PID 2>/dev/null || true; [[ -n "${KEEP:-}" ]] || rm -rf "$tmp"; }
trap cleanup EXIT
port=$((20000 + RANDOM % 20000))
# Honest bots alternate a plain hand and a sharp one (it leads its shots, as a good player
# does); cheats alternate an aim lock and a flick.
# The cheats play kits that shoot: the aim statistics judge shots, and a blade that looses
# thirty bolts in ninety seconds gives them too little to judge (ANTICHEAT.md 9).
aims=""; builds=""
kits=(blade frostweaver shade ironclad)
for i in $(seq 1 "$HONEST"); do
  aims+="$([[ $((i % 2)) == 1 ]] && echo hand || echo sharp),"; builds+="${kits[$(( (i - 1) % 4 ))]},"
done
for i in $(seq 1 "$CHEATS"); do
  aims+="$([[ $((i % 2)) == 1 ]] && echo lock || echo flick),"; builds+="$([[ $((i % 2)) == 1 ]] && echo frostweaver || echo shade),"
done
bots=$((HONEST + CHEATS))
target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$port --cert-out "$tmp/cert.der" \
  --replay-dir "$tmp/replays" --report-secs 5 > "$tmp/zone.log" 2>&1 &
SERVER_PID=$!
sleep 1.5
target/release/gm-bot --connect 127.0.0.1:$port --cert "$tmp/cert.der" --map assets/maps/built/arena.bsp \
  --bots "$bots" --secs "$SECS" --behaviour duelist --builds "${builds%,}" --teams 1,2 \
  --aim "${aims%,}" --report-after 20 > "$tmp/bots.log" 2>&1 || { tail -5 "$tmp/bots.log"; echo "FAIL: the bots did not finish"; exit 1; }
# The fight closes ten seconds after its last hit and is written; then the zone stops (and
# would write what is open anyway).
for _ in $(seq 1 50); do
  sed 's/\x1b\[[0-9;]*m//g' "$tmp/zone.log" | /usr/bin/grep -a "replay written" | /usr/bin/grep -aq "reason=Fight" && break
  sleep 0.5
done
kill -INT $SERVER_PID 2>/dev/null || true; wait $SERVER_PID 2>/dev/null || true
zone_log() { sed 's/\x1b\[[0-9;]*m//g' "$tmp/zone.log"; }

# Every cheat flagged, no hand flagged: the zone's own lines at each bot's leave.
flagged_cheats=0; flagged_honest=0; seen=0
while IFS= read -r line; do
  name="$(echo "$line" | sed -n 's/.* name=\([a-z]*[0-9]*\).*/\1/p')"
  rules="$(echo "$line" | sed -n 's/.* rules=\([^ ]*\).*/\1/p')"
  [[ -n "$name" && -n "$rules" ]] || continue
  seen=$((seen + 1))
  case "$name" in
    # A program is flagged when the rule made for it says so, not any rule.
    lock*|flick*) if [[ ",$rules," == *",${name%%[0-9]*},"* ]]; then flagged_cheats=$((flagged_cheats + 1)); else echo "  not flagged as a ${name%%[0-9]*}: $name: ${line#*aim at leave: }"; fi ;;
    *) if [[ "$rules" != "-" ]]; then flagged_honest=$((flagged_honest + 1)); echo "  flagged: $name: ${line#*aim at leave: }"; fi ;;
  esac
done < <(zone_log | /usr/bin/grep -a "aim at leave")
[[ "$seen" == "$bots" ]] && ok "the zone reported the aim of all $bots bots" || fail "aim lines for $seen of $bots bots"
[[ "$flagged_cheats" == "$CHEATS" ]] && ok "all $CHEATS programs flagged" || fail "$flagged_cheats of $CHEATS programs flagged"
[[ "$flagged_honest" == 0 ]] && ok "none of the $HONEST hands flagged" || fail "$flagged_honest of $HONEST hands flagged"
zone_log | /usr/bin/grep -a "aim at leave" | sed 's/.*aim at leave: //' \
  | awk '{n=""; for(i=1;i<=NF;i++) if($i ~ /^name=/) n=$i; print n, $2, $3, $4, $5, $6, $7, $8, $NF}' | sort | sed 's/^/  /'

# The fight was written, reads back, and its statistics recompute to what the zone said.
files=("$tmp"/replays/*.gmr)
[[ -f "${files[0]}" ]] || { fail "no replay was written"; exit 1; }
fights=0; reports=0; recomputed=0; total_bytes=0; total_secs=0
for f in "${files[@]}"; do
  info="$(target/release/gm-tools replay info "$f")" || { fail "$(basename "$f") does not read"; continue; }
  head="$(echo "$info" | /usr/bin/grep '^replay:')"
  echo "  $(basename "$f"): ${head#replay: }" | cut -c1-200
  if echo "$head" | /usr/bin/grep -q 'reason=Report'; then reports=$((reports + 1)); else
    fights=$((fights + 1))
    total_bytes=$((total_bytes + $(echo "$head" | sed -n 's/.* bytes=\([0-9]*\).*/\1/p')))
    secs="$(echo "$head" | sed -n 's/.* seconds=\([0-9]*\).*/\1/p')"
    if number "$secs"; then total_secs=$((total_secs + secs)); else fail "$(basename "$f"): no length in its header line"; fi
  fi
  # `gm-tools replay aim` against the zone's "replay aim" lines for the same file.
  mine="$(target/release/gm-tools replay aim "$f" | sed 's/^aim: name=//' | sort)"
  theirs="$(zone_log | /usr/bin/grep -a "replay aim: " | /usr/bin/grep -a "$(basename "$f")" \
    | sed -e 's/.*replay aim: //' | awk '{n=""; out=""; for(i=1;i<=NF;i++){ if($i ~ /^name=/) n=substr($i,6); else if($i !~ /^file=/) out=out" "$i } print n out}' | sort)"
  if [[ -n "$mine" && "$mine" == "$theirs" ]]; then recomputed=$((recomputed + 1)); else
    fail "the statistics of $(basename "$f") do not recompute to the zone's"
    diff <(echo "$mine") <(echo "$theirs") | head -6
  fi
done
atleast "$fights" 1 "fights written"
atleast "$reports" 1 "report replays written (a bot reported after 20 s)"
[[ "$recomputed" == "${#files[@]}" ]] && ok "every file's statistics recompute to what the zone said ($recomputed files)" \
  || fail "$recomputed of ${#files[@]} files recompute"
if (( total_secs > 0 )); then
  atmost $(( total_bytes * 60 / total_secs )) "$(budget anticheat max_replay_bytes_per_minute)" "replay bytes per minute of a $bots-player fight"
else
  fail "no fight of any length to measure the bytes per minute on"
fi

# What recording costs: the worst report window with everybody in.
record="$(zone_log | /usr/bin/grep -a "zone report" | /usr/bin/grep -a " players=$bots " | sed -n 's/.* record_us=\([0-9]*\).*/\1/p' | sort -n | tail -1)"
atmost "$record" "$(budget anticheat max_record_us)" "recorder + analysis microseconds per tick ($bots players)"

ring="$(zone_log | /usr/bin/grep -a "zone report" | sed -n 's/.* replay_ring_bytes=\([0-9]*\).*/\1/p' | sort -n | tail -1)"
atmost "$ring" "$(budget anticheat max_ring_bytes)" "bytes of the last half minute the recorder holds ($bots players)"

# The client renders the fight from a program's eyes.
fight="$(for f in "${files[@]}"; do target/release/gm-tools replay info "$f" | /usr/bin/grep -q 'reason=Fight' && { echo "$f"; break; }; done)"
who="$(target/release/gm-tools replay aim "$fight" | /usr/bin/grep -o 'name=lock[0-9]*' | head -1 | cut -d= -f2)"
if target/release/gm-client --headless --software --replay "$fight" --follow "$who" --from 20 --bench 60 \
     --size 640x360 --screenshot "$tmp/replay.ppm" > "$tmp/client.log" 2>&1 && [[ -s "$tmp/replay.ppm" ]]; then
  shades="$(head -c 300000 "$tmp/replay.ppm" | od -An -tu1 -v | tr -s ' ' '\n' | sort -u | wc -l)"
  atleast "$shades" 32 "distinct byte values in the replay's screenshot from $who's eyes (a picture, not a blank)"
else
  tail -5 "$tmp/client.log"; fail "the client did not render the replay"
fi

# 3. The swarm with the recorder on.
if [[ "$SWARM" == 1 ]]; then
  REPLAY=1 SKIP_BUILD=1 scripts/check-swarm.sh || status=1
fi

# 4. Through the hub.
if [[ "$ONLINE" == 1 ]]; then
  : "${GM_TEST_DATABASE_URL:?--online needs GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
  out="$(cargo test -q -p gm-server --test anticheat 2>&1)" || { echo "$out" | tail -25; fail "the hub path"; }
  echo "$out" | /usr/bin/grep -qE 'test result: ok\. 1 passed' \
    && ok "through the hub: statistics, replays, a report upheld, a ban, a trust gate, team kills" \
    || fail "the hub path ran no test"
fi
[[ -n "${KEEP:-}" ]] && echo "logs, replays and the screenshot kept in $tmp"
exit $status
