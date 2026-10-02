#!/usr/bin/env bash
# Web gate (docs/WEB.md 8, PLAN.md 11.8 Phase 8): the browser client.
#
#   scripts/check-web.sh                     build both .wasm, gate their sizes, run the
#                                            transport tests (QUIC and WebTransport in one zone)
#   scripts/check-web.sh --browser           also: an arena zone with a web listener, 15 native
#                                            duelist bots and headless Chromium playing by script,
#                                            once per build (WebGPU, WebGL2)
#   scripts/check-web.sh --browser --software   the same on Chromium's software GPU (CI): the
#                                            frame rate is reported, not gated
#   scripts/check-web.sh --browser --hub     also the whole path: a hub with a web listener, a
#                                            town of 48 bots wearing uploaded avatars, the browser
#                                            logging in, fetching the models into its cache under
#                                            a cap smaller than they are, a second visit served
#                                            from the cache, and a travel to the arena. Needs
#                                            GM_TEST_DATABASE_URL (a Postgres this run wipes).
# Environment: SECS (default 40), SKIP_BUILD=1, CHROME (default chromium), KEEP=DIR (keep logs
# and screenshots there).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BROWSER=0; SOFTWARE=0; HUB=0
for a in "$@"; do
  case "$a" in
    --browser) BROWSER=1 ;;
    --software) SOFTWARE=1 ;;
    --hub) HUB=1 ;;
    *) echo "check-web: unknown argument $a"; exit 2 ;;
  esac
done
[[ "$HUB" == 1 && "$BROWSER" != 1 ]] && { echo "check-web: --hub needs --browser"; exit 2; }
SECS="${SECS:-40}"
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
status=0
# A check whose value or bound is missing fails: an empty string is not a zero.
number() { [[ "$1" =~ ^[0-9]+(\.[0-9]+)?$ ]]; }
check_max() { # value max label
  local v="${1%.*}" m="$2" l="$3"
  if ! number "$1" || ! number "$2"; then echo "FAIL: $l: no number to check ('$1' against '$2')"; status=1
  elif (( v > m )); then echo "FAIL: $l $v exceeds $m"; status=1; else echo "OK: $l $v within $m"; fi
}
check_min() { # value min label
  local v="${1%.*}" m="$2" l="$3"
  if ! number "$1" || ! number "$2"; then echo "FAIL: $l: no number to check ('$1' against '$2')"; status=1
  elif (( v < m )); then echo "FAIL: $l $v is under $m"; status=1; else echo "OK: $l $v at least $m"; fi
}

# 1. Nothing that only compiles for the browser may tell the time through std: it builds and
#    then panics at run time (WEB.md 3.3).
#    The crates the client links are in the search; what is native-only by its cfg is listed.
if /usr/bin/grep -rnE 'std::time::(Instant|SystemTime)|use std::time::\{[^}]*(Instant|SystemTime)' \
     crates/gm-client/src crates/gm-core/src crates/gm-bsp/src crates/gm-model/src crates/gm-hub-proto/src \
     crates/gm-net/src/{bits,client,control,input,quant,snapshot,lib}.rs --include='*.rs' \
     | /usr/bin/grep -vE 'gm-client/src/(headless|net/native|cache/disk)\.rs|gm-client/src/cache\.rs.*SystemTime|gm-client/src/hub\.rs|gm-hub-proto/src/(token|client)\.rs|gm-hub-proto/src/protocol\.rs.*std::time::SystemTime::now\(\)'; then
  echo "FAIL: std::time::Instant / SystemTime in code the browser build compiles (use web_time)"; status=1
else
  echo "OK: no std clock in the code the browser build compiles"
fi

# 2. Build and gate the download.
[[ "${SKIP_BUILD:-}" == 1 && -f target/web/gm-client-webgpu_bg.wasm ]] || scripts/build-web.sh > target/web-build.log 2>&1 \
  || { tail -30 target/web-build.log; echo "FAIL: the web build"; exit 1; }
size() { stat -c %s "$1"; }
packed() { if command -v brotli >/dev/null 2>&1; then brotli -q 11 -c "$1" | wc -c; else gzip -9 -c "$1" | wc -c; fi; }
loader=$(( $(size target/web/boot.js) + $(size target/web/index.html) ))
for name in webgpu webgl; do
  wasm="target/web/gm-client-${name}_bg.wasm"
  check_max "$(size "$wasm")" "$(budget web max_${name}_wasm_bytes)" "$name wasm bytes"
  check_max "$(packed "$wasm")" "$(budget web max_${name}_packed_bytes)" "$name wasm packed bytes"
  check_max "$(( $(size target/web/gm-client-$name.js) + loader ))" "$(budget web max_js_bytes)" "$name JavaScript bytes (glue + loader + page)"
done

# 3. The transports without a browser: a WebTransport session carries what a QUIC connection
#    carries, and a zone takes clients of both kinds at once.
tests() { # label, cargo test arguments: the tests must exist and pass
  local label="$1" out; shift
  out="$(cargo test -q "$@" 2>&1)" || { echo "$out" | tail -20; echo "FAIL: $label"; status=1; return; }
  if echo "$out" | /usr/bin/grep -qE 'test result: ok\. [1-9][0-9]* passed'; then
    echo "OK: $label ($(echo "$out" | /usr/bin/grep -oE '[0-9]+ passed' | head -1))"
  else
    echo "FAIL: $label ran no test"; status=1
  fi
}
tests "a WebTransport session carries streams and datagrams" -p gm-net --features web link::
tests "QUIC and WebTransport clients share a zone" -p gm-server --test loopback

[[ "$BROWSER" == 1 ]] || exit $status

# 4. The acceptance run: a browser client and native clients in the same zone.
command -v node >/dev/null || { echo "check-web: --browser needs node"; exit 1; }
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-server -p gm-bot --locked -q
tmp="${KEEP:-$(mktemp -d)}"; mkdir -p "$tmp"
pids=()
cleanup() {
  kill $SERVER_PID $HTTP_PID $BOTS_PID "${pids[@]:-}" 2>/dev/null || true
  [[ -n "${KEEP:-}" ]] || rm -rf "$tmp"
}
SERVER_PID=; HTTP_PID=; BOTS_PID=
trap cleanup EXIT
http=$((20000 + RANDOM % 20000))
# The page as it was built, served from a directory of this run's own with this run's
# config.json: the build's directory is left as it is (a run that is interrupted leaves
# no test configuration behind in what a deployment copies).
mkdir -p "$tmp/web"; rm -f "$tmp/web"/*
ln -s "$ROOT"/target/web/* "$tmp/web/"; rm -f "$tmp/web/config.json"
echo '{"hub": null, "hub_cert_sha256": null, "dev": true}' > "$tmp/web/config.json"
(cd "$tmp/web" && exec python3 -m http.server "$http" --bind 127.0.0.1 >/dev/null 2>&1) &
HTTP_PID=$!
MAX_BPS="$(budget net max_bytes_per_player_s)"

run() { # build name, query flag
  local build="$1" extra="$2" port=$((20000 + RANDOM % 20000))
  local log="$tmp/zone-$build.log"
  target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$port --cert-out "$tmp/cert.der" \
    --web-listen 127.0.0.1:$((port + 1)) --web-info-out "$tmp/web.json" --report-secs 5 > "$log" 2>&1 &
  SERVER_PID=$!
  sleep 1.5
  target/release/gm-bot --connect 127.0.0.1:$port --cert "$tmp/cert.der" --map assets/maps/built/arena.bsp \
    --bots 15 --secs $((SECS + 12)) --behaviour duelist --builds ironclad,blade,frostweaver,shade --teams 1,2 > "$tmp/bots-$build.log" 2>&1 &
  BOTS_PID=$!
  sleep 2
  local url hash
  url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$tmp/web.json")"
  hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$tmp/web.json")"
  local page="http://127.0.0.1:$http/?connect=$url&cert=$hash&map=arena&name=browser&build=frostweaver&team=1&script=fight&report=1&third-person=1&seconds=$SECS$extra"
  local soft=(); [[ "$SOFTWARE" == 1 ]] && soft=(--software)
  timeout $((SECS + 120)) node scripts/web-run.mjs --url "$page" --seconds "$SECS" --screenshot "$tmp/browser-$build.png" --at $((SECS / 2)) \
    ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$tmp/browser-$build.log" 2>&1 || true
  sleep 1
  kill $BOTS_PID 2>/dev/null || true
  kill -INT $SERVER_PID 2>/dev/null || true; wait $SERVER_PID 2>/dev/null || true
  local done_line leave
  done_line="$(/usr/bin/grep -a '^GM-DONE' "$tmp/browser-$build.log" | tail -1 || true)"
  if [[ -z "$done_line" ]]; then
    tail -15 "$tmp/browser-$build.log"; echo "FAIL: the $build build did not finish its run"; status=1; return
  fi
  echo "$build: $done_line"
  # What the client itself called an error: a browser shows it in its console and plays on.
  if /usr/bin/grep -aq '^\[ERROR\]\|^EXCEPTION' "$tmp/browser-$build.log"; then
    echo "FAIL: the $build build logged an error: $(/usr/bin/grep -a -m1 '^\[ERROR\]\|^EXCEPTION' "$tmp/browser-$build.log" | cut -c1-200)"; status=1
  else
    echo "OK: the $build build logged no error"
  fi
  leave="$(sed 's/\x1b\[[0-9;]*m//g' "$log" | /usr/bin/grep -a 'session at leave' | /usr/bin/grep -a 'name=browser' | tail -1 || true)"
  echo "$build: zone: $(echo "$leave" | sed -n 's/.*\(entity=[0-9]*.*\)/\1/p')"
  f() { echo "$done_line" | sed -n "s/.* $1=\([0-9.A-Za-z]*\).*/\1/p"; }
  z() { echo "$leave" | sed -n "s/.* $1=\([0-9.a-z]*\).*/\1/p"; }
  local want_backend="BrowserWebGpu"; [[ "$build" == webgl ]] && want_backend="Gl"
  [[ "$(f backend)" == "$want_backend" ]] && echo "OK: $build draws through $(f backend)" \
    || { echo "FAIL: $build drew through $(f backend), not $want_backend"; status=1; }
  [[ "$(z web)" == "true" ]] && echo "OK: the zone saw it as a WebTransport session" \
    || { echo "FAIL: the zone has no WebTransport session named browser"; status=1; }
  check_min "$(( $(f snapshots) / SECS ))" "$(budget web min_snapshots_per_s)" "$build snapshots per second"
  check_max "$(f unexplained)" "$(( SECS / 10 + 1 ))" "$build unexplained corrections (netcode budget: one per 10 s)"
  check_min "$(z damage_dealt)" 1 "$build damage dealt by the browser client"
  check_min "$(z damage_taken)" 1 "$build damage taken by the browser client"
  check_max "$(( $(z udp_tx_bytes) / SECS ))" "$MAX_BPS" "$build zone -> browser UDP bytes/s"
  check_max "$(( $(z udp_rx_bytes) / SECS ))" "$MAX_BPS" "$build browser -> zone UDP bytes/s"
  check_max "$(f wasm_memory_bytes)" "$(budget web max_wasm_memory_bytes)" "$build wasm memory bytes"
  check_max "$(f first_frame_ms)" "$(budget web max_first_frame_ms)" "$build ms to the first frame"
  if [[ "$SOFTWARE" == 1 ]]; then
    echo "note: $build fps $(f fps) on the software GPU (not gated)"
  else
    check_min "$(f fps)" "$(budget web min_fps)" "$build frames per second"
  fi
}
run webgpu ""
run webgl "&gl=1"

# 5. The whole path through the hub (WEB.md 4, 8).
hub_run() {
  : "${GM_TEST_DATABASE_URL:?--hub needs GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
  local count=48 cache_mb=16 hp=$((20000 + RANDOM % 20000))
  [[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-hub -p gm-tools --locked -q
  local avatars="target/avatars/$count"
  if [[ ! -f "$avatars/.stamp" || target/release/gm-tools -nt "$avatars/.stamp" ]]; then
    rm -rf "$avatars"; mkdir -p "$avatars"
    target/release/gm-tools model synth --count "$count" --out "$avatars" --ingest > /dev/null
    touch "$avatars/.stamp"
  fi
  target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --wipe --listen 127.0.0.1:$hp --cert-out "$tmp/hub.der" \
    --key "$tmp/hub.key" --zone-secret "$tmp" --models-dir "$tmp/models" --auth-per-minute 100000 \
    --web-listen 127.0.0.1:$((hp + 1)) --web-info-out "$tmp/hub-web.json" > "$tmp/hub.log" 2>&1 &
  pids+=("$!")
  for _ in $(seq 1 50); do [[ -s "$tmp/hub.der" && -s "$tmp/hub-web.json" ]] && break; sleep 0.1; done
  sleep 0.5
  local link=(--hub 127.0.0.1:$hp --hub-cert "$tmp/hub.der")
  target/release/gm-server --map assets/maps/built/town.bsp --listen 127.0.0.1:$((hp + 2)) --cert-out "$tmp/town.der" \
    "${link[@]}" --zone-id town --zone-secret "$tmp" --max-players $((count + 8)) --hz 20 --report-secs 5 \
    --web-listen 127.0.0.1:$((hp + 3)) > "$tmp/town.log" 2>&1 &
  pids+=("$!")
  target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$((hp + 4)) --cert-out "$tmp/arena.der" \
    "${link[@]}" --zone-id arena --zone-secret "$tmp" --report-secs 5 \
    --web-listen 127.0.0.1:$((hp + 5)) > "$tmp/arena.log" 2>&1 &
  pids+=("$!")
  sleep 1
  target/release/gm-tools hub register "${link[@]}" --user moderator@gm.test --password moderator-password > /dev/null
  target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --grant-moderator moderator@gm.test > /dev/null 2>&1
  target/release/gm-tools hub seed-avatars --dir "$avatars" --count "$count" "${link[@]}" \
    --user moderator@gm.test --password moderator-password | tail -1
  target/release/gm-bot "${link[@]}" --user 'avatar-{i}@bots.test' --password avatar-password --character 'Avatar{i}' \
    --zone town --bots "$count" --stalls 6 --secs 120 --behaviour stroll --maps-dir assets/maps/built > "$tmp/town-bots.log" 2>&1 &
  pids+=("$!")
  # The page learns where the hub is from config.json, as a deployment's would.
  local hub_url hub_hash
  hub_url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  hub_hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  echo "{\"hub\": \"$hub_url\", \"hub_cert_sha256\": \"$hub_hash\", \"dev\": true}" > "$tmp/web/config.json"
  for _ in $(seq 1 120); do
    sed 's/\x1b\[[0-9;]*m//g' "$tmp/town.log" | /usr/bin/grep -a "zone report .* players=$count " > /dev/null && break; sleep 0.5
  done
  local soft=(); [[ "$SOFTWARE" == 1 ]] && soft=(--software)
  local base="http://127.0.0.1:$http/?user=web%40gm.test&password=web-password&character=Webby&zone=town&build=blade&report=1&third-person=1&cache-mb=$cache_mb"
  # First visit: nothing cached; every model comes from the hub and the cache must evict.
  # Twenty seconds in, a click on the canvas, as a person's first: the game asks the browser
  # for the pointer on it (the page entered the game by itself, and a browser gives the
  # pointer to a click only: WEB.md 3.4).
  timeout 180 node scripts/web-run.mjs --profile "$tmp/profile" --url "$base&register=1&seconds=30" --seconds 30 --cache gm-models-v1 \
    --click-canvas 20 --screenshot "$tmp/browser-town.png" --at 16 ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$tmp/browser-town.log" 2>&1 || true
  # Second visit, same browser profile: most models come from the cache; then to the arena.
  timeout 180 node scripts/web-run.mjs --profile "$tmp/profile" --url "$base&seconds=24&travel-to=arena&travel-after=12" --seconds 24 --cache gm-models-v1 \
    ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$tmp/browser-travel.log" 2>&1 || true
  local first second
  first="$(/usr/bin/grep -a '^GM-DONE' "$tmp/browser-town.log" | tail -1 || true)"
  second="$(/usr/bin/grep -a '^GM-DONE' "$tmp/browser-travel.log" | tail -1 || true)"
  if [[ -z "$first" || -z "$second" ]]; then
    tail -8 "$tmp/browser-town.log" "$tmp/browser-travel.log"; echo "FAIL: the browser did not finish its visits through the hub"; status=1; return
  fi
  echo "town: $first"; echo "town again, then the arena: $second"
  if /usr/bin/grep -aq '^\[ERROR\]\|^EXCEPTION' "$tmp/browser-town.log" "$tmp/browser-travel.log"; then
    echo "FAIL: the client logged an error through the hub: $(/usr/bin/grep -ah -m1 '^\[ERROR\]\|^EXCEPTION' "$tmp/browser-town.log" "$tmp/browser-travel.log" | head -1 | cut -c1-200)"; status=1
  else
    echo "OK: the client logged no error in the town or on the way to the arena"
  fi
  local holder
  holder="$(sed -n 's/^web-run: after a click the pointer is held by: //p' "$tmp/browser-town.log" | tail -1)"
  if [[ "$holder" == "gm-canvas" ]]; then
    echo "OK: a click on the canvas takes the pointer for the game"
  else
    echo "FAIL: after a click on the canvas the pointer is held by: ${holder:-(the runner did not say)}"; status=1
  fi
  g() { echo "$1" | sed -n "s/.* $2=\([0-9.A-Za-z-]*\).*/\1/p"; }
  # The last report in the town of the second visit (the final line is from the arena).
  local in_town
  in_town="$(/usr/bin/grep -a '^GM-STATS .* zone=town ' "$tmp/browser-travel.log" | tail -1 || true)"
  local cap=$((cache_mb * 1024 * 1024))
  check_min "$(g "$first" models)" "$count" "models drawn in the town"
  check_min "$(g "$first" with_model)" "$count" "characters wearing a model"
  check_min "$(g "$first" fetched)" "$count" "models fetched from the hub on the first visit"
  check_max "$(g "$first" failed)" 0 "models that failed to load"
  check_max "$(g "$first" refused)" 0 "models the client refused"
  check_max "$(g "$first" cache_bytes)" "$cap" "cache bytes as the client counts them, first visit"
  # What the browser really holds: every entry of the cache read back and measured.
  local held1 held2
  held1="$(/usr/bin/grep -a '^GM-CACHE' "$tmp/browser-town.log" | tail -1 || true)"
  held2="$(/usr/bin/grep -a '^GM-CACHE' "$tmp/browser-travel.log" | tail -1 || true)"
  echo "town: $held1"; echo "town again: $held2"
  check_max "$(g "$held1 " bytes)" "$cap" "bytes the browser's cache holds after the first visit"
  check_min "$(g "$held1 " entries)" 20 "models the browser's cache holds after the first visit"
  check_max "$(g "$held2 " bytes)" "$cap" "bytes the browser's cache holds after the second visit"
  check_min "$(g "$in_town" cache_hits)" 20 "models served from the browser cache on the second visit"
  check_max "$(g "$second" cache_bytes)" "$cap" "cache bytes as the client counts them, second visit"
  check_max "$(g "$first" wasm_memory_bytes)" "$(budget web max_wasm_memory_bytes)" "wasm memory bytes in the town"
  [[ "$(g "$second" zone)" == arena ]] && echo "OK: the browser travelled to the arena" \
    || { echo "FAIL: the browser ended in zone '$(g "$second" zone)', not the arena"; status=1; }
  sed 's/\x1b\[[0-9;]*m//g' "$tmp/arena.log" | /usr/bin/grep -a "player joined" | /usr/bin/grep -a "name=Webby .*web=true" > /dev/null \
    && echo "OK: the arena took the character from a WebTransport session" \
    || { echo "FAIL: the arena never saw Webby on a WebTransport session"; status=1; }
  if [[ "$SOFTWARE" != 1 ]]; then
    check_min "$(g "$first" fps)" "$(budget web min_fps)" "frames per second in the town with $count avatars"
  fi
}
if [[ "$HUB" == 1 ]]; then hub_run; fi
[[ -n "${KEEP:-}" ]] && echo "logs and screenshots kept in $tmp"
exit $status
