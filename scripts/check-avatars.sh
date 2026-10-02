#!/usr/bin/env bash
# Avatar gate (MODELS.md 11, PLAN.md 11.8 Phase 6): COUNT distinct avatars at the budget
# ceiling in the town. The client must show every one of them as its own model, hold the frame
# rate and the RSS budget, and its model cache directory must never exceed the cap it was given
# (the cap is smaller than the models, so the run evicts).
#
#   scripts/check-avatars.sh              100 avatars from a directory, windowed, real GPU
#   scripts/check-avatars.sh --gate-fps   also fail below [avatars].min_fps (reference iGPU)
#   scripts/check-avatars.sh --software   48 avatars, headless under a software adapter (CI)
#   scripts/check-avatars.sh --online     the whole path: uploads through the hub (ingestion
#                                         worker, moderation), a town zone, COUNT bots wearing
#                                         the models and opening stalls, the client fetching
#                                         from the hub, and one takedown while it watches.
#                                         Needs GM_TEST_DATABASE_URL (a Postgres this run
#                                         wipes) and a display.
# Environment: COUNT, FRAMES (default 1200), CACHE_MB, HZ (online; default 20), STALLS (online;
# default 12), SKIP_BUILD=1.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
MIN_FPS="$(budget avatars min_fps)"
MAX_P99_MS="$(budget avatars max_frame_ms_p99)"
MAX_RSS="$(budget client max_rss_bytes)"
MAX_GPU_EACH="$(budget avatars max_gpu_bytes_per_model)"
MAX_TOWN_BPS="$(budget avatars max_town_bytes_per_player_s)"
MAP=assets/maps/built/town.bsp

mode=offline; software=0; gate_fps=0
for a in "$@"; do
  case "$a" in
    --software) software=1 ;;
    --gate-fps) gate_fps=1 ;;
    --online) mode=online ;;
    *) echo "usage: $0 [--software] [--gate-fps] [--online]"; exit 2 ;;
  esac
done
if (( software )); then
  COUNT="${COUNT:-$(budget avatars smoke_count)}"; CACHE_MB="${CACHE_MB:-$(budget avatars smoke_cache_cap_mb)}"; FRAMES="${FRAMES:-120}"
else
  COUNT="${COUNT:-$(budget avatars count)}"; CACHE_MB="${CACHE_MB:-$(budget avatars cache_cap_mb)}"; FRAMES="${FRAMES:-1200}"
fi
CAP=$((CACHE_MB * 1024 * 1024))
[[ -f "$MAP" ]] || { echo "check-avatars: $MAP missing (gm-tools map build assets/maps/src/town.map)"; exit 1; }
if [[ "${SKIP_BUILD:-}" != 1 ]]; then
  cargo build --release -p gm-client -p gm-tools --locked -q
  [[ $mode == online ]] && cargo build --release -p gm-hub -p gm-server -p gm-bot --locked -q
fi

# The avatars: generated once per count and kept until the generator changes.
avatars="target/avatars/$COUNT"
if [[ ! -f "$avatars/.stamp" || target/release/gm-tools -nt "$avatars/.stamp" ]]; then
  rm -rf "$avatars"; mkdir -p "$avatars"
  target/release/gm-tools model synth --count "$COUNT" --out "$avatars" --ingest
  touch "$avatars/.stamp"
fi

tmp="$(mktemp -d)"
pids=()
cleanup() { for p in "${pids[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"; }
trap cleanup EXIT
status=0
ok()   { echo "OK: $*"; }
fail() { echo "FAIL: $*"; status=1; }
atmost()  { if (( ${1%.*} > $2 )); then fail "$3 $1 exceeds $2"; else ok "$3 $1 within $2"; fi; }
atleast() { if (( ${1%.*} < $2 )); then fail "$3 $1 below $2"; else ok "$3 $1 at or above $2"; fi; }
equal()   { if [[ "$1" != "$2" ]]; then fail "$3 is $1, expected $2"; else ok "$3 is $1"; fi; }
# field LINE KEY: the value of `KEY=value` on a report line.
field() { echo "$1" | sed -n "s/.*[ :]$2=\([0-9.]*\).*/\1/p"; }

# Run the client in the background and watch its cache directory: the largest size the
# directory ever had is the number the cap is about.
watch_cache() { # pid -> prints the largest size seen
  local pid=$1 worst=0 now
  while kill -0 "$pid" 2>/dev/null; do
    now="$(du -sb "$tmp/cache" 2>/dev/null | cut -f1)"; now="${now:-0}"
    (( now > worst )) && worst=$now
    sleep 0.05
  done
  now="$(du -sb "$tmp/cache" 2>/dev/null | cut -f1)"; now="${now:-0}"
  (( now > worst )) && worst=$now
  echo "$worst"
}

client_checks() { # the client's output, the number of models it must end with
  local out="$1" ready="$2" frames_line avatars_line
  frames_line="$(echo "$out" | grep '^bench: frames=' | tail -1)"
  avatars_line="$(echo "$out" | grep '^avatars: ' | tail -1)"
  [[ -n "$avatars_line" ]] || { echo "$out" | tail -20; echo "FAIL: gm-client did not report on avatars"; exit 1; }
  echo "$out" | grep -E '^bench: (mode|frames|peak)|^avatars: '
  equal "$(field "$avatars_line" models_ready)" "$ready" "distinct models drawn"
  atleast "$(field "$avatars_line" with_model)" "$ready" "characters wearing a model"
  equal "$(field "$avatars_line" failed)" 0 "models that failed to load"
  equal "$(field "$avatars_line" refused)" 0 "models the client refused"
  local gpu; gpu="$(field "$avatars_line" model_gpu_bytes)"
  atmost $((gpu / ready)) "$MAX_GPU_EACH" "GPU bytes per model"
  atmost "$(field "$avatars_line" cache_bytes)" "$CAP" "cache bytes at the end"
  if (( software )); then
    echo "note: RSS is not gated under a software adapter (its video memory is process memory)"
  else
    atmost "$(field "$avatars_line" peak_rss_bytes)" "$MAX_RSS" "peak RSS bytes"
  fi
  if (( gate_fps )); then
    atleast "$(field "$frames_line" fps_avg)" "$MIN_FPS" "average fps"
    atmost "$(field "$frames_line" frame_ms_p99)" "$MAX_P99_MS" "frame time p99 ms"
  fi
}

if [[ $mode == offline ]]; then
  args=(--map "$MAP" --bench "$FRAMES" --no-vsync --crowd "$COUNT" --crowd-dir "$avatars"
        --cache-dir "$tmp/cache" --cache-mb "$CACHE_MB")
  (( software )) && args+=(--headless --software)
  target/release/gm-client "${args[@]}" > "$tmp/client.log" 2>&1 &
  client=$!; pids+=("$client")
  worst="$(watch_cache "$client")"
  wait "$client" || { cat "$tmp/client.log"; echo "FAIL: gm-client exited with an error"; exit 1; }
  client_checks "$(cat "$tmp/client.log")" "$COUNT"
  atmost "$worst" "$CAP" "largest cache directory seen, bytes"
  total="$(find "$avatars" -name '*.gmm' -printf '%s\n' | awk '{s+=$1} END {print s+0}')"
  if (( total > CAP )); then
    kept="$(find "$tmp/cache" -name '*.gmm' | wc -l)"
    if (( kept < COUNT )); then ok "the cache evicted ($kept of $COUNT models kept on disk; $total bytes of models)"; else fail "nothing was evicted although $total bytes of models exceed the cap"; fi
  else
    echo "note: $total bytes of models fit under the cap; nothing had to be evicted"
  fi
  exit $status
fi

# --online
: "${GM_TEST_DATABASE_URL:?--online needs GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
HZ="${HZ:-20}"; STALLS="${STALLS:-12}"; BOT_SECS=60
hub_port=$((20000 + RANDOM % 20000)); zone_port=$((hub_port + 1))
target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --wipe --listen 127.0.0.1:$hub_port \
  --cert-out "$tmp/hub.der" --key "$tmp/hub.key" --zone-secret "$tmp" --models-dir "$tmp/models" \
  --auth-per-minute 100000 > "$tmp/hub.log" 2>&1 &
pids+=("$!")
for _ in $(seq 1 50); do [[ -s "$tmp/hub.der" ]] && break; sleep 0.1; done
sleep 0.5
target/release/gm-server --map "$MAP" --listen 127.0.0.1:$zone_port --cert-out "$tmp/zone.der" \
  --hub 127.0.0.1:$hub_port --hub-cert "$tmp/hub.der" --zone-id town --zone-secret "$tmp" \
  --max-players $((COUNT + 8)) --hz "$HZ" --report-secs 2 > "$tmp/zone.log" 2>&1 &
zone=$!; pids+=("$zone")
sleep 1
hub=(--hub 127.0.0.1:$hub_port --hub-cert "$tmp/hub.der")
target/release/gm-tools hub register "${hub[@]}" --user moderator@gm.test --password moderator-password
target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --grant-moderator moderator@gm.test > /dev/null 2>&1
target/release/gm-tools hub seed-avatars --dir "$avatars" --count "$COUNT" "${hub[@]}" \
  --user moderator@gm.test --password moderator-password
stored="$(find "$tmp/models" -name '*.gmm' | wc -l)"
equal "$stored" "$COUNT" "models the hub ingested and stores"

target/release/gm-bot "${hub[@]}" --user 'avatar-{i}@bots.test' --password avatar-password \
  --character 'Avatar{i}' --zone town --bots "$COUNT" --stalls "$STALLS" --secs "$BOT_SECS" \
  --behaviour stroll --maps-dir assets/maps/built > "$tmp/bots.log" 2>&1 &
bots=$!; pids+=("$bots")
zone_log() { sed 's/\x1b\[[0-9;]*m//g' "$tmp/zone.log"; }
for _ in $(seq 1 120); do zone_log | grep -q "zone report .* players=$COUNT " && break; sleep 0.5; done
zone_log | grep -q "zone report .* players=$COUNT " || { tail -5 "$tmp/bots.log"; echo "FAIL: the $COUNT bots never were all in the town"; exit 1; }

target/release/gm-client "${hub[@]}" --user viewer@gm.test --password viewer-password --register \
  --character Viewer --build blade --zone town --third-person --seconds 25 \
  --cache-dir "$tmp/cache" --cache-mb "$CACHE_MB" > "$tmp/client.log" 2>&1 &
client=$!; pids+=("$client")
# While the client watches, the first bot's model is taken down (MODELS.md 10): everyone must
# stop drawing it at once and the client must delete its copy.
victim="$(target/release/gm-tools model list "${hub[@]}" --user avatar-000@bots.test --password avatar-password | awk 'NR==1 {print $1}')"
[[ ${#victim} == 64 ]] || { echo "FAIL: could not read the first bot's model id"; exit 1; }
( sleep 14
  target/release/gm-tools mod takedown "$victim" --code copyright --reason "check-avatars.sh" --reference GATE \
    "${hub[@]}" --user moderator@gm.test --password moderator-password > "$tmp/takedown.log" 2>&1 ) &
worst="$(watch_cache "$client")"
wait "$client" || { cat "$tmp/client.log"; echo "FAIL: gm-client exited with an error"; exit 1; }
cat "$tmp/takedown.log"
client_checks "$(cat "$tmp/client.log")" $((COUNT - 1))
atmost "$worst" "$CAP" "largest cache directory seen, bytes"
grep 'net:' "$tmp/client.log" | sed 's/^.*net: /net: /'
line="$(grep '^avatars: ' "$tmp/client.log" | tail -1)"
equal "$(field "$line" fetched)" "$COUNT" "models fetched from the hub"
if [[ -e "$tmp/cache/$victim.gmm" ]]; then fail "the taken-down model is still in the client's cache"; else ok "the taken-down model is gone from the client's cache"; fi

# The zone's last window with everyone in: its bytes per player.
report="$(zone_log | grep -E "zone report .* players=$((COUNT + 1)) " | tail -1)"
[[ -n "$report" ]] || { echo "FAIL: no zone report with $((COUNT + 1)) players"; exit 1; }
echo "$report" | sed 's/^.*zone report/zone report/' | cut -c1-200
atmost "$(field "$report" tx_bps)" "$MAX_TOWN_BPS" "zone tx bytes/player/s at $HZ Hz"
equal "$(field "$report" overruns)" 0 "zone tick overruns"

wait "$bots" || { tail -5 "$tmp/bots.log"; echo "FAIL: bots did not finish"; exit 1; }
summary="$(sed 's/\x1b\[[0-9;]*m//g' "$tmp/bots.log" | grep '^hub bots=' | tail -1)"
echo "$summary"
equal "$(field "$summary" completed)" "$COUNT" "bots that completed"
equal "$(field "$summary" wearing_a_model)" $((COUNT - 1)) "bots still wearing their model after the takedown"
equal "$(field "$summary" models_seen_max)" "$COUNT" "distinct models each bot was told about"
equal "$(field "$summary" revocations_max)" 1 "takedowns every bot heard"
equal "$(field "$summary" stalls_opened)" "$STALLS" "stalls opened"
equal "$(field "$summary" stalls_seen_max)" "$STALLS" "stalls every bot saw"
# The netcode budget (PROTOCOL.md 9): fewer than one unexplained correction per 10 s per bot.
atmost "$(field "$summary" unexplained_max)" $((BOT_SECS / 10)) "unexplained corrections of the worst bot in $BOT_SECS s"
exit $status
