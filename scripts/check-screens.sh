#!/usr/bin/env bash
# Screens gate (docs/CLIENT.md 10, PLAN.md 11.8 Phase 10): the client's screens, used by
# somebody who is not a person.
#
#   scripts/check-screens.sh             the toolkit, the screens, the menu, the chat, UI
#                                        scripts, settings and the font as tests (every screen
#                                        whole at every window size), the players' messages and
#                                        the rules for names; with GM_TEST_DATABASE_URL also
#                                        what the screens lean on at the hub and in a zone
#   scripts/check-screens.sh --desktop   also the desktop client on a display of its own (Xvfb,
#                                        the software GPU), started from another directory with
#                                        a settings file that names only the hub. By UI script:
#                                        a new account, a new character, into the town, a line
#                                        heard from a bot and one said to it, the menu and its
#                                        pages, to the arena and back and there again, Leave,
#                                        and rounds of Play and Leave. Then by real input
#                                        (xdotool): the remembered email, the password pasted
#                                        from the clipboard, Enter, a click on Play, Escape, a
#                                        click on Quit. Then a client with no hub named anywhere.
#   scripts/check-screens.sh --browser   also both browser builds in headless Chromium: the
#                                        page's own form filled by the browser's input events,
#                                        then the screens on the canvas by UI script
#                                        (--software: on Chromium's software GPU, as CI has it)
# --desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes).
# Environment: ROUNDS (of Play and Leave; default from budgets.toml), SKIP_BUILD=1,
# SKIP_TESTS=1 (the tests of the first line were run already), CHROME (default chromium),
# KEEP=DIR (keep logs and screenshots there).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
DESKTOP=0; BROWSER=0; SOFTWARE="${SOFTWARE:-0}"
for a in "$@"; do
  case "$a" in
    --desktop) DESKTOP=1 ;;
    --browser) BROWSER=1 ;;
    --software) SOFTWARE=1 ;;
    *) echo "check-screens: unknown argument $a"; exit 2 ;;
  esac
done
budget() { awk -v sec="[$1]" -v key="$2" '/^\[/{s=$1} s==sec && $1==key {print $3; exit}' budgets.toml; }
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
ROUNDS="${ROUNDS:-$(budget screens rounds)}"
[[ "$ROUNDS" =~ ^[1-9][0-9]*$ ]] || { echo "check-screens: rounds must be a number of one or more ('$ROUNDS')"; exit 2; }
tests() { # minimum, label, cargo test arguments: the tests must exist and pass
  local min="$1" label="$2" out passed; shift 2
  out="$(cargo test -q "$@" 2>&1)" || { echo "$out" | tail -25; fail "$label"; return; }
  passed="$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | sort -n | tail -1)"
  atleast "${passed:-0}" "$min" "$label: tests passed"
}

# 1. Without a display: the screens drawn into a recorder at five window sizes, the state
#    machine against a scripted hub, the script reader, the settings file, the font.
if [[ "${SKIP_TESTS:-}" != 1 ]]; then
  tests 13 "the toolkit (widgets, focus, layout at every size)" -p gm-client ui::
  tests 13 "the screens before the game, against a scripted hub" -p gm-client front::
  tests 5 "the menu, its pages and the chat" -p gm-client menu::
  tests 2 "UI scripts" -p gm-client script::
  tests 5 "the settings file" -p gm-client settings::
  tests 2 "the font (every glyph, no two alike)" -p gm-model smallfont::
  tests 2 "names (what every client can draw, nothing another could be taken for)" -p gm-hub-proto names::
  tests 1 "the players' messages are the hub's own requests" -p gm-hub-proto player::
  tests 1 "an account's chat bucket" -p gm-server --lib chat_bucket
  if [[ -n "${GM_TEST_DATABASE_URL:-}" ]]; then
    tests 1 "what the screens lean on at the hub and in a zone" -p gm-server --test screens
    tests 1 "chat is checked and limited where it arrives" -p gm-server --test loopback chat_is_checked_and_limited
  else
    echo "note: GM_TEST_DATABASE_URL is not set: the hub's side of the screens was not run"
  fi
fi
[[ "$DESKTOP" == 1 || "$BROWSER" == 1 ]] || exit $status

# 2. A hub, a town and an arena for the clients below.
: "${GM_TEST_DATABASE_URL:?--desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-hub -p gm-server -p gm-bot -p gm-client --locked -q
tmp="${KEEP:-$(mktemp -d)}"; mkdir -p "$tmp"; tmp="$(cd "$tmp" && pwd)"; mkdir -p "$tmp/elsewhere"
# (A directory that is kept may hold an earlier run's files: none of them is this run's.)
rm -f "$tmp/hub.der" "$tmp/hub-web.json" "$tmp"/*.log "$tmp/display"
pids=()
cleanup() {
  kill "${pids[@]:-}" 2>/dev/null || true
  [[ -n "${KEEP:-}" ]] || rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
plain() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
# The last lines of a log, for whoever reads why it failed.
show() { echo "--- the end of $(basename "$1"):"; plain "$1" 2>/dev/null | tail -"${2:-12}" || true; }
# A program just started is still there, or the run ends here with its last words.
started() { # pid, name, log
  if ! kill -0 "$1" 2>/dev/null; then show "$3"; echo "FAIL: $2 did not start"; exit 1; fi
}
hp=$((20000 + RANDOM % 20000))
target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL" --wipe --listen 127.0.0.1:$hp --cert-out "$tmp/hub.der" \
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
target/release/gm-server --map assets/maps/built/arena.bsp --listen 127.0.0.1:$((hp + 4)) --cert-out "$tmp/arena.der" \
  "${link[@]}" --zone-id arena --zone-secret "$tmp" --report-secs 5 \
  --web-listen 127.0.0.1:$((hp + 5)) > "$tmp/arena.log" 2>&1 &
arena_pid=$!; pids+=("$arena_pid")
# Both zones are up when each has written its certificate and is ticking.
for _ in $(seq 1 100); do
  if [[ -s "$tmp/town.der" && -s "$tmp/arena.der" ]] \
    && /usr/bin/grep -aq "zone running" "$tmp/town.log" && /usr/bin/grep -aq "zone running" "$tmp/arena.log"; then break; fi
  sleep 0.1
done
started $hub_pid "the hub" "$tmp/hub.log"
started $town_pid "the town" "$tmp/town.log"
started $arena_pid "the arena" "$tmp/arena.log"
# Somebody in the town who says a line every few seconds, and says what it hears as it
# hears it.
crier_pid=
crier() { # name, seconds, log
  target/release/gm-bot "${link[@]}" --user "$1@bots.test" --password crier-password --register --character "$1" \
    --zone town --bots 1 --secs "$2" --behaviour hold --maps-dir assets/maps/built \
    --say "hello from a bot" --say-every 3 > "$3" 2>&1 &
  crier_pid=$!; pids+=("$crier_pid")
}
# Wait until the bot's log says it heard `text` (it logs a line when it hears one); then
# the bot has done its part.
heard() { # log, text, label
  local found=0
  for _ in $(seq 1 200); do
    if plain "$1" | /usr/bin/grep -a "heard" | /usr/bin/grep -aF "text=$2" > /dev/null; then found=1; break; fi
    kill -0 "$crier_pid" 2>/dev/null || break
    sleep 0.1
  done
  kill "$crier_pid" 2>/dev/null || true
  if [[ "$found" == 1 ]]; then ok "$3"; else show "$1" 4; fail "$3: it did not"; fi
}
# The time (epoch seconds, to the millisecond) at which a log first shows a line, while
# the program that writes it lives.
stamp() { # log, text, pid
  for _ in $(seq 1 6000); do
    if /usr/bin/grep -aqF "$2" "$1" 2>/dev/null; then date +%s.%3N; return 0; fi
    kill -0 "$3" 2>/dev/null || return 1
    sleep 0.05
  done
  return 1
}
since() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", b - a }'; }

# What the client itself called an error: a browser shows it in its console and plays on
# (the WebGL build drew no town for two phases, and said so there).
quiet() { # log, label
  local first; first="$(/usr/bin/grep -a -m1 '^\[ERROR\]\|^EXCEPTION' "$1" || true)"
  if [[ -z "$first" ]]; then ok "$2: the client logged no error"; else fail "$2: the client logged an error: ${first:0:200}"; fi
}

desktop() {
  command -v Xvfb >/dev/null || { fail "--desktop needs Xvfb"; return; }
  command -v xdotool >/dev/null || { fail "--desktop needs xdotool"; return; }
  [[ "${SKIP_BUILD:-}" == 1 && -x target/release/examples/clipboard ]] \
    || cargo build --release -p gm-client --example clipboard --locked -q
  # A display of its own: what is typed and clicked below never reaches a person's desktop.
  # The server picks the number and says it; whatever else the session has (a Wayland
  # compositor) is kept from the programs started on it.
  Xvfb -displayfd 3 -screen 0 1280x720x24 -nolisten tcp 3> "$tmp/display" > "$tmp/xvfb.log" 2>&1 &
  local xvfb=$!; pids+=("$xvfb")
  for _ in $(seq 1 100); do [[ -s "$tmp/display" ]] && break; kill -0 $xvfb 2>/dev/null || break; sleep 0.1; done
  [[ -s "$tmp/display" ]] || { show "$tmp/xvfb.log"; fail "Xvfb did not start"; return; }
  local disp; disp="$(head -1 "$tmp/display")"
  local on=(env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET "DISPLAY=:$disp")
  local client=("$ROOT/target/release/gm-client" --software --settings "$tmp/settings.toml")
  printf 'hub = "127.0.0.1:%s"\nhub_cert = "%s"\n' "$hp" "$tmp/hub.der" > "$tmp/settings.toml"

  # 3. A cold start, by script. Every line waits for what it needs, so the script is also
  #    the proof that each screen came up and said what it should.
  {
    cat <<'EOF'
wait screen login
click "New account"
field email
type screens@gm.test
field password
type a long password
field "password again"
type a long password
click "Create account"
wait screen new character
field name
type Aldric
click frostweaver
expect "Ice from a distance"
click Create
wait screen characters
expect "Aldric  frostweaver  new"
click Play
wait screen game
say in the town
expect "Crier: hello from a bot"
key Enter
wait screen chat
type hello from the screens
key Enter
wait screen game
expect "Aldric: hello from the screens"
key Escape
wait screen menu
click Keys
wait screen keys
expect "say something"
click Back
wait screen menu
click Settings
wait screen settings
click "size of text 2"
click Back
wait screen menu
click Travel
wait screen travel
click "arena  arena"
click Go
wait screen game
expect "arena  Esc menu"
key Escape
wait screen menu
click Travel
wait screen travel
expect "arena  arena  here"
click "town  town"
click Go
wait screen game
expect "town  Esc menu"
key Escape
wait screen menu
click Travel
wait screen travel
expect "town  town  here"
click "arena  arena"
click Go
wait screen game
expect "arena  Esc menu"
key Escape
wait screen menu
click Travel
wait screen travel
expect "arena  arena  here"
click Back
wait screen menu
click Leave
wait screen characters
expect "Aldric  frostweaver  arena"
say rounds begin
EOF
    for _ in $(seq 1 "$ROUNDS"); do
      printf 'click Play\nwait screen game\nkey Escape\nwait screen menu\nclick Leave\nwait screen characters\n'
    done
    printf 'say rounds end\nexpect "Aldric  frostweaver  arena"\nsleep 1\nquit\n'
  } > "$tmp/cold.ui"
  crier Crier 90 "$tmp/crier.log"
  local t0 pid t_town t_r0 t_r1
  t0="$(date +%s.%3N)"
  (cd "$tmp/elsewhere" && exec "${on[@]}" timeout 600 "${client[@]}" --ui-script "$tmp/cold.ui") > "$tmp/cold.log" 2>&1 &
  pid=$!; pids+=("$pid")
  # What the client holds before the rounds and after them, and how many threads it has:
  # a round that left something behind (a connection's thread, a map) would show here.
  of() { awk -v key="$2:" '$1 == key { print $2 }' "/proc/$(pgrep -P "$1" -x gm-client | head -1)/status" 2>/dev/null || true; }
  local rss0 rss1 threads0 threads1
  t_town="$(stamp "$tmp/cold.log" "ui-script: in the town" $pid)" || t_town=""
  t_r0="$(stamp "$tmp/cold.log" "ui-script: rounds begin" $pid)" || t_r0=""
  rss0="$(of $pid VmRSS)"; threads0="$(of $pid Threads)"
  t_r1="$(stamp "$tmp/cold.log" "ui-script: rounds end" $pid)" || t_r1=""
  rss1="$(of $pid VmRSS)"; threads1="$(of $pid Threads)"
  if wait $pid && /usr/bin/grep -aq '^ui-script: ok' "$tmp/cold.log"; then
    ok "a cold start by script: account, character, town, chat, menu, arena, town, arena, Leave, $ROUNDS rounds of Play and Leave"
  else
    show "$tmp/cold.log"
    fail "the cold start by script did not reach its end"
    return
  fi
  if [[ -n "$t_town" ]]; then
    atmost "$(since "$t0" "$t_town")" "$(budget screens max_secs_to_town)" "seconds from the program's start to standing in the town (software GPU)"
  else
    fail "the way into the town was not timed"
  fi
  if [[ -n "$t_r0" && -n "$t_r1" ]]; then
    local per; per="$(awk -v a="$t_r0" -v b="$t_r1" -v n="$ROUNDS" 'BEGIN { printf "%.0f", (b - a) * 1000 / n }')"
    atmost "$per" "$(budget screens max_round_ms)" "milliseconds per round of Play and Leave ($ROUNDS rounds)"
  else
    fail "the rounds were not timed"
  fi
  if number "${rss0:-}" && number "${rss1:-}" && number "${threads0:-}" && number "${threads1:-}"; then
    local grown=$(( rss1 > rss0 ? (rss1 - rss0) * 1024 : 0 ))
    atmost "$grown" "$(budget screens max_rounds_rss_growth_bytes)" "bytes of RSS the $ROUNDS rounds added (from $((rss0 * 1024)))"
    atmost "$threads1" "$((threads0 + $(budget screens max_rounds_threads_added)))" "threads after the $ROUNDS rounds ($threads0 before)"
  else
    fail "the client's memory and threads were not read around the rounds ('${rss0:-}', '${rss1:-}', '${threads0:-}', '${threads1:-}')"
  fi
  # What was remembered: the address, the character played last, what was chosen.
  for line in 'email = "screens@gm.test"' 'character = "Aldric"' 'third_person = true' 'ui_scale = 2'; do
    if /usr/bin/grep -qF "$line" "$tmp/settings.toml"; then ok "the settings remember: $line"; else fail "the settings do not say: $line"; fi
  done
  if /usr/bin/grep -qF "a long password" "$tmp/settings.toml" "$tmp/cold.log"; then
    fail "the password is in the settings file or in the client's log"
  else
    ok "the password is neither in the settings file nor in the client's log"
  fi
  heard "$tmp/crier.log" "hello from the screens" "the bot in the town heard the line the client said"
  local joins; joins="$(plain "$tmp/arena.log" | /usr/bin/grep -a "player joined" | /usr/bin/grep -ac "name=Aldric" || true)"
  atleast "$joins" $((ROUNDS + 2)) "times the arena took Aldric"

  # 4. The same client again, by real input: what the window system delivers, not what a
  #    script hands in. The script only says when the screen is ready and where to click.
  cat > "$tmp/real.ui" <<'EOF'
wait screen login
expect "email: screens@gm.test"
click "show the password"
field password
say paste
expect "password: a long password"
say pasted
wait screen characters
expect "Aldric  frostweaver  arena"
where Play
wait screen game
say playing
wait screen menu
where Quit
EOF
  "${on[@]}" target/release/examples/clipboard "a long password" 120 > "$tmp/clipboard.log" 2>&1 &
  pids+=("$!")
  (cd "$tmp/elsewhere" && exec "${on[@]}" timeout 180 "${client[@]}" --ui-script "$tmp/real.ui") > "$tmp/real.log" 2>&1 &
  pid=$!; pids+=("$pid")
  local x=("${on[@]}" xdotool) at
  told() { stamp "$tmp/real.log" "ui-script: $1" $pid > /dev/null; }
  spot() { /usr/bin/grep -a "^ui-script: where [0-9]* [0-9]* $1\$" "$tmp/real.log" | tail -1 | awk '{print $3, $4}' || true; }
  real() {
    told "paste" || return 1
    # No window manager is here to give the window the keyboard.
    "${x[@]}" windowfocus "$("${x[@]}" search --name '^gamengine' | head -1)"
    sleep 0.2
    "${x[@]}" key ctrl+v
    told "pasted" || return 1
    "${x[@]}" key Return
    told "where " || return 1
    at="$(spot Play)"; [[ -n "$at" ]] || return 1
    # shellcheck disable=SC2086
    "${x[@]}" mousemove $at
    "${x[@]}" click 1
    told "playing" || return 1
    "${x[@]}" key Escape
    for _ in $(seq 1 600); do at="$(spot Quit)"; [[ -n "$at" ]] && break; kill -0 $pid 2>/dev/null || return 1; sleep 0.05; done
    [[ -n "$at" ]] || return 1
    # shellcheck disable=SC2086
    "${x[@]}" mousemove $at
    "${x[@]}" click 1
  }
  if real && wait $pid; then
    ok "by real input: the email remembered, the password pasted, Enter, a click on Play, Escape, a click on Quit"
  else
    kill $pid 2>/dev/null || true
    show "$tmp/real.log"
    fail "the run by real input did not reach its end"
  fi

  # 5. No hub named anywhere: the client says so, and the offline walk is one click away.
  if [[ -e "$ROOT/client.toml" ]]; then
    echo "note: this build ships a client.toml (a hub of its own): the screen for no hub was not run"
  else
    : > "$tmp/nohub.toml"
    printf 'wait screen title\nexpect "No hub is named"\nclick "Walk around offline"\nwait screen game\nkey Escape\nwait screen menu\nclick Quit\nsleep 30\n' > "$tmp/nohub.ui"
    if (cd "$tmp/elsewhere" && "${on[@]}" timeout 120 "$ROOT/target/release/gm-client" --software \
         --settings "$tmp/nohub.toml" --ui-script "$tmp/nohub.ui") > "$tmp/nohub.log" 2>&1; then
      ok "with no hub named: the screen that says so, the offline walk, the menu, Quit"
    else
      show "$tmp/nohub.log"; fail "the client with no hub named did not end well"
    fi
  fi
}

browser() {
  command -v node >/dev/null || { fail "--browser needs node"; return; }
  [[ "${SKIP_BUILD:-}" == 1 && -f target/web/gm-client-webgpu_bg.wasm ]] || scripts/build-web.sh > target/web-build.log 2>&1 \
    || { tail -30 target/web-build.log; fail "the web build"; return; }
  # The page as it was built, served from a directory of this run's own with this run's
  # config.json: the build's directory is left as it is.
  local http=$((20000 + RANDOM % 20000)) hub_url hub_hash
  mkdir -p "$tmp/web"; rm -f "$tmp/web"/*
  ln -s "$ROOT"/target/web/* "$tmp/web/"; rm -f "$tmp/web/config.json"
  hub_url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  hub_hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  echo "{\"hub\": \"$hub_url\", \"hub_cert_sha256\": \"$hub_hash\", \"dev\": true}" > "$tmp/web/config.json"
  (cd "$tmp/web" && exec python3 -m http.server "$http" --bind 127.0.0.1 >/dev/null 2>&1) &
  pids+=("$!")
  local soft=(); [[ "$SOFTWARE" == 1 ]] && soft=(--software)
  local build name script
  for build in webgpu webgl; do
    name="Web$build"
    # The form is the page's (filled below by the browser's own input events); what
    # follows it is drawn on the canvas.
    script="$(printf '%s\n' \
      'wait screen new character' 'field name' "type $name" 'click blade' 'click Create' \
      'wait screen characters' "expect \"$name  blade  new\"" 'click Play' 'wait screen game' \
      "expect \"Crier$build: hello from a bot\"" 'key Enter' 'wait screen chat' "type hello from $build" 'key Enter' \
      'wait screen game' 'key Escape' 'wait screen menu' 'click Travel' 'wait screen travel' \
      'click "arena  arena"' 'click Go' 'wait screen game' 'expect "arena  Esc menu"' 'key Escape' 'wait screen menu' \
      'click Travel' 'wait screen travel' 'expect "arena  arena  here"' 'click Back' 'wait screen menu' \
      'click Leave' 'wait screen characters' "expect \"$name  blade  arena\"" 'quit')"
    local query; query="$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$script")"
    local extra=""; [[ "$build" == webgl ]] && extra="&gl=1"
    crier "Crier$build" 150 "$tmp/crier-$build.log"
    timeout 240 node scripts/web-run.mjs --url "http://127.0.0.1:$http/?ui-script=$query$extra" --seconds 120 \
      --login "$build@gm.test" --password "a long password" --register \
      --screenshot "$tmp/browser-$build.png" ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$tmp/browser-$build.log" 2>&1 || true
    if /usr/bin/grep -aq '^GM-DONE ui-script: ok' "$tmp/browser-$build.log"; then
      ok "$build: the page's form by the browser's input, then the screens on the canvas: character, town, chat, arena, Leave"
    else
      show "$tmp/browser-$build.log"
      fail "the $build build did not reach the end of its script"
      kill "$crier_pid" 2>/dev/null || true
      continue
    fi
    if /usr/bin/grep -aq "^GM-BUILD $build" "$tmp/browser-$build.log"; then ok "$build: that build ran"; else fail "$build: another build ran"; fi
    quiet "$tmp/browser-$build.log" "$build, in the town and the arena"
    if plain "$tmp/arena.log" | /usr/bin/grep -a "player joined" | /usr/bin/grep -a "name=$name .*web=true" > /dev/null; then
      ok "$build: the arena took $name from a WebTransport session"
    else
      fail "$build: the arena never saw $name on a WebTransport session"
    fi
    heard "$tmp/crier-$build.log" "hello from $build" "$build: the bot in the town heard the line the browser said"
  done
}

[[ "$DESKTOP" == 1 ]] && desktop
[[ "$BROWSER" == 1 ]] && browser
[[ -n "${KEEP:-}" ]] && echo "logs and screenshots kept in $tmp"
exit $status
