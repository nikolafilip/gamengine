#!/usr/bin/env bash
# The look gate (docs/LOOK.md 8, docs/CONTENT.md 5, PLAN.md 11.8 Phase 14): the content
# standard built and reproduced, props in hands, the skin and its grids, the hotbar.
#
#   scripts/check-look.sh              the tables and their looks checked, the bundle rebuilt
#                                      into a temporary directory and compared with the
#                                      committed one byte for byte, the unit tests of the
#                                      format, the ingestion of props, the baked icons, the
#                                      toolkit's grids and the screens; the offline town with
#                                      an armed crowd against the same crowd unarmed (the
#                                      prop draw cost), on the software GPU unless --gate-fps
#   scripts/check-look.sh --gate-fps   the offline runs on the real GPU with the fps budget
#   scripts/check-look.sh --desktop    also the desktop client on a display of its own (Xvfb,
#                                      the software GPU): a hub and a town, a character made
#                                      by script and handed a sword and a cuirass; by UI
#                                      script it opens the inventory, drags the sword onto
#                                      the weapon slot (the zone answers worn), hovers for a
#                                      tooltip, and its hotbar and held prop are read from
#                                      its report; a screenshot of each page is kept
#   scripts/check-look.sh --browser    the same by the browser build in headless Chromium
#                                      (--software: on Chromium's software GPU)
# --desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes).
# Environment: SKIP_BUILD=1, SKIP_TESTS=1, CHROME (default chromium), KEEP=DIR (keep logs
# and screenshots there).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
DESKTOP=0; BROWSER=0; GATE_FPS=0; SOFTWARE="${SOFTWARE:-0}"
for a in "$@"; do
  case "$a" in
    --desktop) DESKTOP=1 ;;
    --browser) BROWSER=1 ;;
    --gate-fps) GATE_FPS=1 ;;
    --software) SOFTWARE=1 ;;
    *) echo "check-look: unknown argument $a"; exit 2 ;;
  esac
done
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
status=0
ok()   { echo "OK: $*"; }
fail() { echo "FAIL: $*"; status=1; }
number() { [[ "$1" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; }
atmost() { # value max label
  if ! number "$1" || ! number "$2"; then fail "$3: no number to check ('$1' against '$2')"
  elif awk -v a="$1" -v b="$2" 'BEGIN { exit !(a > b) }'; then fail "$3 $1 exceeds $2"; else ok "$3 $1 within $2"; fi
}
atleast() {
  if ! number "$1" || ! number "$2"; then fail "$3: no number to check ('$1' against '$2')"
  elif awk -v a="$1" -v b="$2" 'BEGIN { exit !(a < b) }'; then fail "$3 $1 below $2"; else ok "$3 $1 at least $2"; fi
}
equal() { if [[ "$1" == "$2" ]]; then ok "$3: $1"; else fail "$3: '$1', not '$2'"; fi; }
field() { echo "$1" | sed -n "s/.* $2=\([-0-9.a-zA-Z_:,\/]*\).*/\1/p" | head -1; }
tests() { # minimum, label, cargo test arguments: the tests must exist and pass
  local min="$1" label="$2" out passed; shift 2
  out="$(cargo test -q "$@" 2>&1)" || { echo "$out" | tail -25; fail "$label"; return; }
  passed="$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | sort -n | tail -1)"
  atleast "${passed:-0}" "$min" "$label: tests passed"
}
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-tools -p gm-client --locked -q
tmp="${KEEP:-$(mktemp -d)}"; mkdir -p "$tmp"; tmp="$(cd "$tmp" && pwd)"; mkdir -p "$tmp/elsewhere"
rm -f "${tmp:?}/hub.der" "${tmp:?}/hub-web.json" "${tmp:?}"/*.log "${tmp:?}/display"
pids=()
cleanup() {
  kill "${pids[@]:-}" 2>/dev/null || true
  [[ -n "${KEEP:-}" ]] || rm -rf "${tmp:?}"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
plain() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
show() { echo "--- the end of $(basename "$1"):"; plain "$1" 2>/dev/null | tail -"${2:-12}" || true; }

# 1. The content: checked, and the committed bundle is what the sources build (CONTENT.md 5.4).
if target/release/gm-tools content check --built assets/built/content > "$tmp/content.log" 2>&1; then
  ok "the content: $(tail -2 "$tmp/content.log" | head -1 | sed 's/^content: //')"
  ok "the committed bundle is what the sources build, byte for byte"
else
  cat "$tmp/content.log" | tail -20; fail "the content check (run gm-tools content build and commit what it writes)"
fi
bundle_bytes="$(find assets/built/content -type f -printf '%s\n' | awk '{s+=$1} END {print s+0}')"
atmost "$bundle_bytes" "$(budget content max_bundle_bytes)" "bundle bytes"
atmost "$(stat -c %s assets/built/content/ui.gma)" "$(budget content max_atlas_bytes)" "atlas bytes"
for f in assets/built/content/props/*.gmm; do
  atmost "$(stat -c %s "$f")" "$(budget content max_prop_gmm_bytes)" "$(basename "$f") bytes"
done

# 2. Without a display.
if [[ "${SKIP_TESTS:-}" != 1 ]]; then
  tests 3 "the .gmm prop kind, the atlas and the manifest" -p gm-model prop
  tests 2 "the atlas format round trip and its bounds" -p gm-model atlas::
  tests 1 "the manifest" -p gm-model manifest::
  tests 5 "props ingested (flat colours to swatches, a fit, a reach refused), icons and portraits baked" -p gm-ingest --test ingest prop
  tests 2 "the looks of the content: props by first appearance, what a body holds" -p gm-content looks::
  tests 6 "the inventory, the storage, the stall and the trade as grids, against a scripted hub" -p gm-client bag::
  tests 3 "the toolkit's scale rule and wrapping" -p gm-client ui::tests::
  tests 2 "UI scripts: hover, drag, expect image" -p gm-client script::
fi

# 3. The offline town: a crowd armed against the same crowd bare (the prop draw cost).
COUNT=48; FRAMES=240
avatars="$tmp/avatars"
if [[ -z "${AVATARS:-}" ]]; then
  target/release/gm-tools model synth --count "$COUNT" --out "$avatars" --ingest --side 256 > "$tmp/synth.log" 2>&1 \
    || { tail -5 "$tmp/synth.log"; fail "the synthetic crowd"; }
else
  avatars="$AVATARS"
fi
bench() { # label, extra arguments...; prints the frames line
  local label="$1"; shift
  local args=(--map assets/maps/built/town.bsp --bench "$FRAMES" --no-vsync --crowd "$COUNT" --crowd-dir "$avatars"
              --cache-dir "$tmp/cache" --cache-mb 64 --headless "$@")
  (( GATE_FPS )) || args+=(--software)
  if target/release/gm-client "${args[@]}" > "$tmp/bench-$label.log" 2>&1; then
    /usr/bin/grep -a '^bench: frames=' "$tmp/bench-$label.log" | tail -1
  else
    tail -5 "$tmp/bench-$label.log"; fail "the offline bench ($label)"; echo ""
  fi
}
bare="$(bench bare)"
armed="$(bench armed --prop sword)"
if [[ -n "$bare" && -n "$armed" ]]; then
  cost="$(awk -v a="$(field "$bare" frame_ms_avg)" -v b="$(field "$armed" frame_ms_avg)" 'BEGIN { printf "%.3f", b - a }')"
  echo "look: crowd=$COUNT bare_frame_ms=$(field "$bare" frame_ms_avg) armed_frame_ms=$(field "$armed" frame_ms_avg) prop_cost_ms=$cost"
  atmost "$cost" "$(budget look max_prop_draw_ms)" "milliseconds a frame $COUNT held props add"
  if (( GATE_FPS )); then
    atleast "$(field "$armed" fps_avg)" "$(budget look min_fps)" "average fps with every body armed"
  fi
fi
[[ "$DESKTOP" == 1 || "$BROWSER" == 1 ]] || exit $status

# 4. A hub and a town.
: "${GM_TEST_DATABASE_URL:?--desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-hub -p gm-server -p gm-bot --locked -q
started() { if ! kill -0 "$1" 2>/dev/null; then show "$3"; echo "FAIL: $2 did not start"; exit 1; fi; }
stamp() { # log, text, pid
  for _ in $(seq 1 6000); do
    if /usr/bin/grep -aqF "$2" "$1" 2>/dev/null; then date +%s.%3N; return 0; fi
    kill -0 "$3" 2>/dev/null || return 1
    sleep 0.05
  done
  return 1
}
hp=$((20000 + RANDOM % 20000))
hub=(target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL")
"${hub[@]}" --wipe --listen 127.0.0.1:$hp --cert-out "$tmp/hub.der" \
  --key "$tmp/hub.key" --zone-secret "$tmp" --models-dir "$tmp/models" --auth-per-minute 100000 --start-zone town \
  --web-listen 127.0.0.1:$((hp + 1)) --web-info-out "$tmp/hub-web.json" > "$tmp/hub.log" 2>&1 &
hub_pid=$!; pids+=("$hub_pid")
for _ in $(seq 1 150); do [[ -s "$tmp/hub.der" && -s "$tmp/hub-web.json" ]] && break; kill -0 $hub_pid 2>/dev/null || break; sleep 0.1; done
[[ -s "$tmp/hub.der" && -s "$tmp/hub-web.json" ]] || { show "$tmp/hub.log"; echo "FAIL: the hub did not come up"; exit 1; }
sleep 0.5
link=(--hub 127.0.0.1:$hp --hub-cert "$tmp/hub.der")
target/release/gm-server --map assets/maps/built/town.bsp --listen 127.0.0.1:$((hp + 2)) --cert-out "$tmp/town.der" \
  "${link[@]}" --zone-id town --zone-secret "$tmp" --hz 20 --report-secs 5 \
  --web-listen 127.0.0.1:$((hp + 3)) > "$tmp/town.log" 2>&1 &
town_pid=$!; pids+=("$town_pid")
running=0
for _ in $(seq 1 150); do
  if [[ -s "$tmp/town.der" ]] && /usr/bin/grep -aq "zone running" "$tmp/town.log"; then running=1; break; fi
  kill -0 $town_pid 2>/dev/null || break
  sleep 0.1
done
started $hub_pid "the hub" "$tmp/hub.log"
started $town_pid "the town" "$tmp/town.log"
[[ "$running" == 1 ]] || { show "$tmp/town.log"; echo "FAIL: the town did not come up"; exit 1; }
# A walker with a hammer build: somebody else to see holding a prop.
target/release/gm-bot "${link[@]}" --user walker@bots.test --password walker-password --register --character Walker \
  --zone town --bots 1 --builds ironclad --secs 600 --behaviour hold --maps-dir assets/maps/built > "$tmp/walker.log" 2>&1 &
pids+=("$!")
# What the operator does for a character made by a client: a sword, a cuirass, coin.
provide() { # character
  for item in "sword core/iron,frame/oak" "dagger core/iron,frame/oak" "cuirass core/iron,frame/oak"; do
    # shellcheck disable=SC2086
    "${hub[@]}" --grant-item "$1" $item > "$tmp/grant.log" 2>&1 || { show "$tmp/grant.log"; fail "an item for $1"; return 1; }
  done
  "${hub[@]}" --grant-coin "$1" 150 > "$tmp/grant.log" 2>&1 || { show "$tmp/grant.log"; fail "coin for $1"; return 1; }
}
# What the character does once it stands in the game: the inventory as a grid, the sword
# dragged onto the weapon slot, the tooltip, the equip panel.
dressing() {
  cat <<'EOS'
wait screen game
say in the town
key I
wait screen inventory
expect image item/sword
expect "weapon: nothing"
say inventory
drag "dagger  slash +2.0%" weapon
expect "weapon: dagger"
expect "dagger  slash +2.0%  worn"
say worn
hover "cuirass  physical -2.0%"
expect "tooltip: cuirass"
say tooltip
key Escape
wait screen game
say armed
sleep 3
EOS
}
quiet() { # log, label
  local first; first="$(/usr/bin/grep -a -m1 '^\[ERROR\]\|^EXCEPTION' "$1" || true)"
  if [[ -z "$first" ]]; then ok "$2: the client logged no error"; else fail "$2: the client logged an error: ${first:0:200}"; fi
}
# The last report of a run: the hotbar's cells, what is held and how many props loaded.
reported() { # log, label
  local line; line="$(plain "$1" | /usr/bin/grep -a 'hotbar=' | tail -1)"
  [[ -n "$line" ]] || { fail "$2: no report with a hotbar"; return; }
  local bar; bar="$(field "$line" hotbar)"
  if [[ "$bar" == *"LMB:sword:"* ]]; then ok "$2: the hotbar's first cell is the sword"; else fail "$2: the hotbar: $bar"; fi
  atleast "$(field "$line" held)" 2 "$2: bodies holding something (the character and the walker)"
  atleast "$(field "$line" props_loaded)" 2 "$2: props loaded from the bundle"
}

desktop() {
  command -v Xvfb >/dev/null || { fail "--desktop needs Xvfb"; return; }
  Xvfb -displayfd 3 -screen 0 1280x720x24 -nolisten tcp 3> "$tmp/display" > "$tmp/xvfb.log" 2>&1 &
  local xvfb=$!; pids+=("$xvfb")
  for _ in $(seq 1 100); do [[ -s "$tmp/display" ]] && break; kill -0 $xvfb 2>/dev/null || break; sleep 0.1; done
  [[ -s "$tmp/display" ]] || { show "$tmp/xvfb.log"; fail "Xvfb did not start"; return; }
  local disp; disp="$(head -1 "$tmp/display")"
  local on=(env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET "DISPLAY=:$disp")
  local client=("$ROOT/target/release/gm-client" --software --settings "$tmp/settings.toml" --report)
  printf 'hub = "127.0.0.1:%s"\nhub_cert = "%s"\nmute = true\nthird_person = true\n' "$hp" "$tmp/hub.der" > "$tmp/settings.toml"
  cat > "$tmp/new.ui" <<'EOS'
wait screen login
click "New account"
field email
type dresser@gm.test
field password
type a long password
field "password again"
type a long password
click "Create account"
wait screen new character
field name
type Dresser
click blade
click Create
wait screen characters
expect "Dresser  blade  new"
quit
EOS
  if (cd "$tmp/elsewhere" && "${on[@]}" timeout 120 "${client[@]}" --ui-script "$tmp/new.ui") > "$tmp/new.log" 2>&1; then
    ok "a new account and a character called Dresser, by script"
  else
    show "$tmp/new.log"; fail "the character was not made"; return
  fi
  provide Dresser || return
  { cat <<'EOS'
wait screen login
expect "email: dresser@gm.test"
field password
type a long password
key Enter
wait screen characters
click Play
EOS
    dressing; echo quit; } > "$tmp/dress.ui"
  local pid
  (cd "$tmp/elsewhere" && exec "${on[@]}" timeout 180 "${client[@]}" --ui-script "$tmp/dress.ui") > "$tmp/dress.log" 2>&1 &
  pid=$!; pids+=("$pid")
  # A screenshot at every `say`, for a person to look at.
  local shots=0
  for name in "in the town" inventory worn tooltip armed; do
    if stamp "$tmp/dress.log" "ui-script: $name" $pid > /dev/null; then
      sleep 0.8
      "${on[@]}" xwd -root -silent -out "$tmp/${name// /-}.xwd" 2>/dev/null \
        && ffmpeg -loglevel error -y -i "$tmp/${name// /-}.xwd" "$tmp/${name// /-}.png" 2>/dev/null && rm -f "$tmp/${name// /-}.xwd" && shots=$((shots + 1))
    fi
  done
  if wait $pid && /usr/bin/grep -aq '^ui-script: ok' "$tmp/dress.log"; then
    ok "by script: the inventory as a grid of pictures, the dagger dragged onto the weapon slot and worn, the tooltip, the equip panel ($shots screenshots)"
  else
    show "$tmp/dress.log"; fail "the dressing by script did not reach its end"; return
  fi
  quiet "$tmp/dress.log" "the desktop client"
  reported "$tmp/dress.log" "the desktop client"
  local told; told="$(plain "$tmp/town.log" | /usr/bin/grep -ac " gear " || true)"
  atleast "$told" 1 "gear readings the zone applied"
  # A blade's bare hand holds the sword; the dagger worn is another look, told to everyone.
  told="$(plain "$tmp/town.log" | /usr/bin/grep -ac ' look$\| look ' || true)"
  atleast "$told" 1 "looks the zone announced on a change of gear"
}

browser() {
  command -v node >/dev/null || { fail "--browser needs node"; return; }
  [[ "${SKIP_BUILD:-}" == 1 && -f target/web/gm-client-webgpu_bg.wasm ]] || scripts/build-web.sh > target/web-build.log 2>&1 \
    || { tail -30 target/web-build.log; fail "the web build"; return; }
  [[ -s target/web/assets/built/content/manifest.gmc ]] && ok "the web build carries the bundle" || fail "the web build has no bundle"
  atmost "$(stat -c %s target/web/gm-client-webgpu_bg.wasm)" "$(budget web max_webgpu_wasm_bytes)" "WebGPU wasm bytes"
  local http=$((20000 + RANDOM % 20000)) hub_url hub_hash
  mkdir -p "$tmp/web"; rm -f "${tmp:?}/web"/*
  ln -s "$ROOT"/target/web/* "$tmp/web/"; rm -f "${tmp:?}/web/config.json"
  hub_url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  hub_hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  echo "{\"hub\": \"$hub_url\", \"hub_cert_sha256\": \"$hub_hash\", \"dev\": true}" > "$tmp/web/config.json"
  (cd "$tmp/web" && exec python3 -m http.server "$http" --bind 127.0.0.1 >/dev/null 2>&1) &
  pids+=("$!")
  local soft=(); [[ "$SOFTWARE" == 1 ]] && soft=(--software)
  local build name script query
  page() { # log, script, more arguments of web-run
    local log="$1"; query="$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$2")"; shift 2
    timeout 240 node scripts/web-run.mjs --url "http://127.0.0.1:$http/?ui-script=$query&report=1&third-person=1" --seconds 120 \
      --login "$build@gm.test" --password "a long password" "$@" ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$log" 2>&1 || true
    /usr/bin/grep -aq '^GM-DONE ui-script: ok' "$log"
  }
  build=webgpu; name="Dresser$build"
  script="$(printf '%s\n' 'wait screen new character' 'field name' "type $name" 'click blade' 'click Create' \
    'wait screen characters' "expect \"$name  blade  new\"" 'quit')"
  if page "$tmp/browser-new.log" "$script" --register; then
    ok "$build: a new account by the page's form and a character called $name"
  else
    show "$tmp/browser-new.log"; fail "the $build build did not make its character"; return
  fi
  provide "$name" || return
  script="$(printf '%s\n' 'wait screen characters' 'click Play'; dressing; echo quit)"
  if page "$tmp/browser.log" "$script" --screenshot "$tmp/browser.png"; then
    ok "$build: the inventory as a grid, the dagger dragged onto its slot and worn, the tooltip"
  else
    show "$tmp/browser.log"; fail "the $build build did not reach the end of its dressing"; return
  fi
  quiet "$tmp/browser.log" "$build, in the town"
  reported "$tmp/browser.log" "the browser"
}

[[ "$DESKTOP" == 1 ]] && desktop
[[ "$BROWSER" == 1 ]] && browser
[[ -n "${KEEP:-}" ]] && echo "logs and screenshots kept in $tmp"
exit $status
