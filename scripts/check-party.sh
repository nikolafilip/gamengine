#!/usr/bin/env bash
# People gate (docs/PARTY.md 9, PLAN.md 11.8 Phase 12): parties of people, their lines, a
# trade between two, the tavern.
#
#   scripts/check-party.sh             the split with people in it, a squad that follows its
#                                      commander's party, the wire (two directions, two
#                                      types; a health that is no longer sent), the zone's
#                                      mirror of the hub's parties, the screens (every page
#                                      whole at every window size) and the chat's channels;
#                                      with GM_TEST_DATABASE_URL also the hub (invitations
#                                      and their limits, numbered readings whatever races,
#                                      the sweep, lines to where they are heard) and two
#                                      zones with clients driven by hand (a party across
#                                      zones, a trade asked standing together, a fight
#                                      whose roster is closed)
#   scripts/check-party.sh --online    also, by somebody who is not a person: two bots form
#                                      a party in the town, leave, meet again in the
#                                      tutorial dungeon, clear it together, and the hub
#                                      splits what it drops between them
#   scripts/check-party.sh --desktop   also the desktop client on a display of its own (Xvfb,
#                                      the software GPU): by UI script a new character asks
#                                      one of the two into a party, says a party's line and
#                                      a whisper and reads the answers, buys what the other
#                                      got in the dungeon through the trade window, and
#                                      hires an avatar in the tavern
#   scripts/check-party.sh --browser   the same in both browser builds in headless Chromium
#                                      (--software: on Chromium's software GPU)
# --online, --desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes);
# --desktop and --browser run the bots of --online first.
# Environment: SKIP_BUILD=1, SKIP_TESTS=1, CHROME (default chromium), KEEP=DIR (keep logs
# and screenshots there). QUICK=1 leaves the dungeon out (for whoever works on the screens'
# part: the one in the town then sells an operator's dagger, and nothing of the two bots'
# run is checked).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ONLINE=0; DESKTOP=0; BROWSER=0; SOFTWARE="${SOFTWARE:-0}"
for a in "$@"; do
  case "$a" in
    --online) ONLINE=1 ;;
    --desktop) DESKTOP=1; ONLINE=1 ;;
    --browser) BROWSER=1; ONLINE=1 ;;
    --software) SOFTWARE=1 ;;
    *) echo "check-party: unknown argument $a"; exit 2 ;;
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
equal() { if [[ "$1" == "$2" ]]; then ok "$3: $1"; else fail "$3: '$1', not '$2'"; fi; }
tests() { # minimum, label, cargo test arguments: the tests must exist and pass
  local min="$1" label="$2" out passed; shift 2
  out="$(cargo test -q "$@" 2>&1)" || { echo "$out" | tail -25; fail "$label"; return; }
  passed="$(echo "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | sort -n | tail -1)"
  atleast "${passed:-0}" "$min" "$label: tests passed"
}

# 1. Without a display.
if [[ "${SKIP_TESTS:-}" != 1 ]]; then
  tests 3 "two humans at one boss: one party to the ledger and both paid; two parties; a squad follows its commander's party" -p gm-ai --test party
  tests 4 "the ledger: a commander and its squad, people who tank and heal, healing's credit, a party that goes out" -p gm-core encounter::
  tests 8 "the wire: the handshake's bytes with two types, the reach of a trade, a health that is no longer sent" -p gm-net -- control:: snapshot::
  tests 2 "the zone's mirror of the hub's parties: the largest number wins in any order, a body that comes again, the repair" -p gm-server --lib party::
  tests 4 "the people, the trade and the tavern, against a scripted hub: what was not agreed to is marked until it is, and only what was looked at can be accepted" -p gm-client people::
  tests 1 "the chat's channels and commands; nothing said aloud looks like a whisper" -p gm-client the_chat
  tests 1 "the players' messages are the hub's own requests" -p gm-hub-proto player::
  if [[ -n "${GM_TEST_DATABASE_URL:-}" ]]; then
    tests 1 "the hub's parties: invitations and their limits, readings in order whatever races, away and out, lines to where they are heard" -p gm-hub --test party
    tests 15 "the economy's transactions (a hire names its price and keeps its build)" -p gm-hub --test economy
    tests 1 "the economy over the wire: a trade opened by the zone both play in, and by no other" -p gm-hub --test economy_protocol
    tests 1 "two zones and clients by hand: a party across zones, lines, a trade asked standing together, a fight whose roster is closed, nobody back into a fight they left" -p gm-server --test party
  else
    echo "note: GM_TEST_DATABASE_URL is not set: the hub's and the zones' side of parties was not run"
  fi
fi
[[ "$ONLINE" == 1 ]] || exit $status

# 2. A hub, a town and the tutorial dungeon.
: "${GM_TEST_DATABASE_URL:?--online, --desktop and --browser need GM_TEST_DATABASE_URL (a Postgres this run wipes)}"
[[ "${SKIP_BUILD:-}" == 1 ]] || cargo build --release -p gm-hub -p gm-server -p gm-bot -p gm-client --locked -q
tmp="${KEEP:-$(mktemp -d)}"; mkdir -p "$tmp"; tmp="$(cd "$tmp" && pwd)"; mkdir -p "$tmp/elsewhere"
# (A directory that is kept may hold an earlier run's files: none of them is this run's.
# KEEP is a directory for this gate's logs, not a shared one.)
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
started() { # pid, name, log
  if ! kill -0 "$1" 2>/dev/null; then show "$3"; echo "FAIL: $2 did not start"; exit 1; fi
}
# Wait (so many seconds) until a log shows a line, while the program that writes it lives.
shown() { # log, text, pid, seconds
  for _ in $(seq 1 $(( ${4:-30} * 10 ))); do
    if plain "$1" 2>/dev/null | /usr/bin/grep -aqF "$2"; then return 0; fi
    kill -0 "$3" 2>/dev/null || return 1
    sleep 0.1
  done
  return 1
}
hp=$((20000 + RANDOM % 20000))
# (A member that left the game is of its party for twenty seconds here, two minutes in
# production: long enough for the two to meet again in the dungeon, short enough to see
# the hub let go of the one who stays away.)
AWAY=20
hub=(target/release/gm-hub --database-url "$GM_TEST_DATABASE_URL")
"${hub[@]}" --wipe --listen 127.0.0.1:$hp --cert-out "$tmp/hub.der" \
  --key "$tmp/hub.key" --zone-secret "$tmp" --models-dir "$tmp/models" --auth-per-minute 100000 --start-zone town \
  --party-away $AWAY --web-listen 127.0.0.1:$((hp + 1)) --web-info-out "$tmp/hub-web.json" > "$tmp/hub.log" 2>&1 &
hub_pid=$!; pids+=("$hub_pid")
for _ in $(seq 1 150); do [[ -s "$tmp/hub.der" && -s "$tmp/hub-web.json" ]] && break; kill -0 $hub_pid 2>/dev/null || break; sleep 0.1; done
[[ -s "$tmp/hub.der" && -s "$tmp/hub-web.json" ]] || { show "$tmp/hub.log"; echo "FAIL: the hub did not come up"; exit 1; }
sleep 0.5
link=(--hub 127.0.0.1:$hp --hub-cert "$tmp/hub.der")
target/release/gm-server --map assets/maps/built/town.bsp --listen 127.0.0.1:$((hp + 2)) --cert-out "$tmp/town.der" \
  "${link[@]}" --zone-id town --zone-secret "$tmp" --hz 20 --report-secs 5 \
  --web-listen 127.0.0.1:$((hp + 3)) > "$tmp/town.log" 2>&1 &
town_pid=$!; pids+=("$town_pid")
RECRUITS=ironclad,mender,frostweaver
target/release/gm-server --map assets/maps/built/dungeon.bsp --listen 127.0.0.1:$((hp + 4)) --cert-out "$tmp/dungeon.der" \
  "${link[@]}" --zone-id dungeon --zone-secret "$tmp" --squads --recruits "$RECRUITS" --arrive-at-entry \
  --report-secs 5 > "$tmp/dungeon.log" 2>&1 &
dungeon_pid=$!; pids+=("$dungeon_pid")
for log in town dungeon; do
  for _ in $(seq 1 150); do
    /usr/bin/grep -aq "zone running" "$tmp/$log.log" 2>/dev/null && break
    sleep 0.1
  done
  /usr/bin/grep -aq "zone running" "$tmp/$log.log" || { show "$tmp/$log.log"; echo "FAIL: the $log did not come up"; exit 1; }
done
started $hub_pid "the hub" "$tmp/hub.log"
started $town_pid "the town" "$tmp/town.log"
started $dungeon_pid "the dungeon" "$tmp/dungeon.log"

bot=(target/release/gm-bot "${link[@]}" --maps-dir assets/maps/built --bots 1)
ana=("${bot[@]}" --user ana@bots.test --password ana-password --character Ana --builds blade)
bojan=("${bot[@]}" --user bojan@bots.test --password bojan-password --character Bojan --builds blade)
# Three owners list an avatar each in the tavern and stay away.
HIRE=150
"${bot[@]}" --bots 3 --user 'owner-{i}@bots.test' --password owner-password --register \
  --character 'Avatar{i}' --builds "$RECRUITS" --zone town --list-for-hire $HIRE --secs 0 > "$tmp/owners.log" 2>&1 \
  || { show "$tmp/owners.log"; echo "FAIL: the owners could not list"; exit 1; }
equal "$(plain "$tmp/owners.log" | sed -n 's/^hub bots=3 completed=\([0-9]*\).*/\1/p')" 3 "avatars listed in the tavern"

field() { echo "$1" | sed -n "s/.* $2=\([^ ]*\).*/\1/p"; }
# The party a bot was told it is of: `first`, as it was made (the leader's name first), or
# `last`, as it was at the end, its members in the alphabet's order (the one who led may
# have left the game a moment before the other, and the hub's sweep hands the lead on).
party_of() { # log, first | last
  local all; all="$(plain "$1" | sed -n 's/.* party name=[A-Za-z0-9]* party=\(.*\)$/\1/p')"
  if [[ "$2" == first ]]; then echo "$all" | /usr/bin/grep -a . | head -1
  else echo "$all" | tail -1 | tr ',' '\n' | sort | paste -sd, -; fi
}
KILL=30
books() { # label, coin that moved in trades, hires
  local line; line="$("${hub[@]}" --audit 2>/dev/null | /usr/bin/grep -a "^audit:" || true)"
  [[ -n "$line" ]] || { fail "the hub's audit said nothing"; return; }
  echo "$line"
  equal "$(echo "$line" | sed -n 's/.* unsound=\([0-9-]*\).*/\1/p')" 0 "$1: balances that disagree with the ledger"
  [[ "$KILL" == 0 ]] || equal "$(echo "$line" | sed -n 's/.* drop=\([0-9]*\).*/\1/p')" "$KILL" "$1: coin from the kill"
  [[ -z "${2:-}" ]] || equal "$(echo "$line" | sed -n 's/.* trade=\([0-9]*\).*/\1/p')" "$2" "$1: coin that changed hands in trades"
  [[ -z "${3:-}" ]] || equal "$(echo "$line" | sed -n 's/.* hire=\([0-9]*\).*/\1/p')" "$3" "$1: coin the avatars' owners were paid"
}
if [[ "${QUICK:-}" == 1 ]]; then
  KILL=0
  "${bojan[@]}" --register --zone town --secs 2 --behaviour hold > "$tmp/bojan-town.log" 2>&1 \
    || { show "$tmp/bojan-town.log"; echo "FAIL: the one in the town could not be made"; exit 1; }
else
  # 3. Two people (bots) in the town: one asks the other into a party, the other joins
  #    (the one who invites asks every five seconds until it is in a party: three
  #    chances in its stay, the first of them a second after the other came).
  "${bojan[@]}" --register --zone town --secs 18 --behaviour hold --sociable > "$tmp/bojan-town.log" 2>&1 &
  bojan_pid=$!; pids+=("$bojan_pid")
  sleep 1
  "${ana[@]}" --register --zone town --secs 16 --behaviour hold --invite Bojan > "$tmp/ana-town.log" 2>&1 \
    || { show "$tmp/ana-town.log"; fail "the one who invites did not finish its stay in the town"; }
  wait $bojan_pid || { show "$tmp/bojan-town.log"; fail "the one who joins did not finish its stay in the town"; }
  equal "$(party_of "$tmp/ana-town.log" first)" "Ana,Bojan" "the party as the one who invited was told it (it leads)"
  equal "$(party_of "$tmp/bojan-town.log" first)" "Ana,Bojan" "the party as the one who joined was told it"

  # 4. Both left the game; they meet again in the dungeon within the time a party waits,
  #    and are of it there. They clear it together, each with the zone's recruits, and the
  #    hub splits what the Warden drops between them.
  "${bojan[@]}" --zone dungeon --secs 420 --behaviour raid --sociable > "$tmp/bojan-dungeon.log" 2>&1 &
  bojan_pid=$!; pids+=("$bojan_pid")
  ana_ok=1
  "${ana[@]}" --zone dungeon --secs 420 --behaviour raid > "$tmp/ana-dungeon.log" 2>&1 || ana_ok=0
  bojan_ok=1; wait $bojan_pid || bojan_ok=0
  [[ "$ana_ok" == 1 && "$bojan_ok" == 1 ]] || { show "$tmp/ana-dungeon.log" 5; show "$tmp/bojan-dungeon.log" 5; fail "the two did not both finish the dungeon"; }
  both=0; coins=0; drops=0
  for who in ana bojan; do
    raid="$(plain "$tmp/$who-dungeon.log" | /usr/bin/grep -a '^raid ' | tail -1)"
    echo "$raid" | cut -c1-200
    equal "$(party_of "$tmp/$who-dungeon.log" last)" "Ana,Bojan" "$who: of the party in the dungeon, as the claim said"
    equal "$(field "$raid" done)" true "$who: the dungeon was finished"
    equal "$(field "$raid" cleared | sed 's/:[0-9]*s//g')" "[gate,warden]" "$who: encounters cleared"
    loot="$(field "$raid" loot | tr -d '[]')"
    n=0; [[ -n "$loot" ]] && n="$(echo "$loot" | tr ',' '\n' | wc -l)"
    atleast "$n" 1 "$who: components from the Warden"
    c="$(field "$raid" coin)"; number "$c" || c=0
    drops=$((drops + n)); coins=$((coins + c))
    equal "$(field "$raid" trials_passed)" "[]" "$who: trials passed (they are for one player and a squad)"
    if plain "$tmp/$who-dungeon.log" | /usr/bin/grep -aq "a party of people passes no trial"; then
      both=$((both + 1))
    fi
  done
  equal "$both" 2 "people told at the pull that a party passes no trial"
  equal "$drops" 3 "components the Warden dropped, split between the two"
  equal "$coins" 30 "silver the Warden dropped, split between the two"
  secs="$(field "$(plain "$tmp/ana-dungeon.log" | /usr/bin/grep -a '^raid ' | tail -1)" secs)"
  atmost "$secs" "$(budget party max_secs_dungeon_together)" "seconds two people and their squads took for the dungeon"
  books "after the dungeon"
fi
[[ "$DESKTOP" == 1 || "$BROWSER" == 1 ]] || { [[ -n "${KEEP:-}" ]] && echo "logs kept in $tmp"; exit $status; }

# 5. One of the two goes back to the town and stays: it joins whoever invites, trades with
#    whoever asks (one thing it carries, for three silver), and answers what is said to it.
PRICE=3; PURSE=50
SPOT_BOT="272,-320,25 180"; SPOT="200,-320,25 0"
place() { # character, "x,y,z yaw": the zone puts a character away a moment after it left
  local out=""
  for _ in $(seq 1 50); do
    # shellcheck disable=SC2086
    if out="$("${hub[@]}" --place "$1" town ${2% *} "${2#* }" 2>&1)"; then return 0; fi
    sleep 0.2
  done
  echo "$out" | tail -3; fail "$1 was not placed in the town"; return 1
}
place Bojan "$SPOT_BOT" || exit 1
"${bojan[@]}" --zone town --secs 1500 --behaviour hold --sociable --trade-for $PRICE > "$tmp/bojan.log" 2>&1 &
bojan_pid=$!; pids+=("$bojan_pid")
if [[ "${QUICK:-}" == 1 ]]; then
  "${hub[@]}" --grant-item Bojan dagger core/iron,frame/oak > "$tmp/grant.log" 2>&1 || { show "$tmp/grant.log"; fail "a dagger for the one in the town"; }
  thing=dagger
else
  # The other stays away: the hub's sweep takes it out of the party, and one is no party.
  # (Whoever of the two the hub has leading by now.)
  if { shown "$tmp/bojan.log" " party name=Bojan party=Ana,Bojan" $bojan_pid 15 \
       || plain "$tmp/bojan.log" | /usr/bin/grep -aqF " party name=Bojan party=Bojan,Ana"; } \
     && shown "$tmp/bojan.log" "the party is no more" $bojan_pid $((AWAY + 25)); then
    ok "somebody who stays away is let go of: the one who is left is in no party"
  else
    show "$tmp/bojan.log"; fail "the hub did not let go of the member that stayed away"
  fi
  # What the one in the town got from the Warden: what it will offer (the newest thing it
  # carries: the last of its share).
  drop="$(field "$(plain "$tmp/bojan-dungeon.log" | /usr/bin/grep -a '^raid ' | tail -1)" loot | tr -d '[]' | tr ',' '\n' | tail -1)"
  thing="${drop#*/}"; thing="${thing//_/ }"
  [[ -n "$thing" ]] || { fail "the one in the town carries nothing from the dungeon"; thing="nothing"; }
fi

# What the operator does for a character made by a client: coin, and a place beside the
# other.
provide() { # character
  place "$1" "$SPOT" || return 1
  "${hub[@]}" --grant-coin "$1" $PURSE > "$tmp/grant.log" 2>&1 || { show "$tmp/grant.log"; fail "coin for $1"; return 1; }
}
# What a person does about other people, one line a step: a party, a party's line and a
# whisper, a trade for what the other carries, a hire. `me` is the character's own name,
# `thing` what the other offers.
together() { # me, thing
  cat <<EOS
wait screen game
key P
wait screen people
expect "you are in no party"
click "Bojan"
click Invite
expect "your party: $1, Bojan"
expect "Bojan  party"
say in a party
key Escape
wait screen game
key Enter
wait screen chat
type /p ready?
key Enter
expect "[party] $1: ready?"
expect "[party] Bojan: aye"
key Enter
wait screen chat
type /w Bojan psst
key Enter
expect "[to Bojan] psst"
expect "[whisper] Bojan: psst yourself"
key P
wait screen people
click "Bojan"
click Trade
wait screen trade
expect "Bojan gives"
expect "$2"
expect "new"
click silver
type 3
click "Set coin"
expect "the coin is set"
click Accept
expect "the trade is done"
say traded
click Close
wait screen game
key I
wait screen inventory
expect "$2"
key Escape
wait screen game
key P
wait screen people
click Tavern
wait screen tavern
expect "nothing is refunded"
click "Avatar000"
expect "Avatar000 for"
click Hire
expect "hired: it joins your squad at the next zone you enter"
click Back
wait screen people
click "Bojan"
click Leave
expect "you are in no party"
key Escape
wait screen game
EOS
}
# What the client itself called an error.
quiet() { # log, label
  [[ -s "$1" ]] || { fail "$2: the client left no log"; return; }
  local first; first="$(/usr/bin/grep -a -m1 '^\[ERROR\]\|^EXCEPTION' "$1" || true)"
  if [[ -z "$first" ]]; then ok "$2: the client logged no error"; else fail "$2: the client logged an error: ${first:0:200}"; fi
}
# The time (epoch seconds, to the millisecond) at which a log first shows a line.
stamp() { # log, text, pid
  for _ in $(seq 1 6000); do
    if /usr/bin/grep -aqF "$2" "$1" 2>/dev/null; then date +%s.%3N; return 0; fi
    kill -0 "$3" 2>/dev/null || return 1
    sleep 0.05
  done
  return 1
}
since() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", b - a }'; }
done_n=0

desktop() {
  command -v Xvfb >/dev/null || { fail "--desktop needs Xvfb"; return; }
  Xvfb -displayfd 3 -screen 0 1280x720x24 -nolisten tcp 3> "$tmp/display" > "$tmp/xvfb.log" 2>&1 &
  local xvfb=$!; pids+=("$xvfb")
  for _ in $(seq 1 100); do [[ -s "$tmp/display" ]] && break; kill -0 $xvfb 2>/dev/null || break; sleep 0.1; done
  [[ -s "$tmp/display" ]] || { show "$tmp/xvfb.log"; fail "Xvfb did not start"; return; }
  local disp; disp="$(head -1 "$tmp/display")"
  local on=(env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET "DISPLAY=:$disp")
  local client=("$ROOT/target/release/gm-client" --software --settings "$tmp/settings.toml")
  printf 'hub = "127.0.0.1:%s"\nhub_cert = "%s"\n' "$hp" "$tmp/hub.der" > "$tmp/settings.toml"
  cat > "$tmp/new.ui" <<'EOS'
wait screen login
click "New account"
field email
type cvita@gm.test
field password
type a long password
field "password again"
type a long password
click "Create account"
wait screen new character
field name
type Cvita
click blade
click Create
wait screen characters
expect "Cvita  blade  new"
quit
EOS
  if (cd "$tmp/elsewhere" && "${on[@]}" timeout 120 "${client[@]}" --ui-script "$tmp/new.ui") > "$tmp/new.log" 2>&1; then
    ok "a new account and a character called Cvita, by script"
  else
    show "$tmp/new.log"; fail "the character was not made"; return
  fi
  quiet "$tmp/new.log" "the desktop client, making its character"
  provide Cvita || return
  { cat <<'EOS'
wait screen login
expect "email: cvita@gm.test"
field password
type a long password
key Enter
wait screen characters
click Play
EOS
    together Cvita "$thing"; echo quit; } > "$tmp/together.ui"
  local pid t_party t_traded
  (cd "$tmp/elsewhere" && exec "${on[@]}" timeout 240 "${client[@]}" --ui-script "$tmp/together.ui") > "$tmp/together.log" 2>&1 &
  pid=$!; pids+=("$pid")
  t_party="$(stamp "$tmp/together.log" "ui-script: in a party" $pid)" || t_party=""
  t_traded="$(stamp "$tmp/together.log" "ui-script: traded" $pid)" || t_traded=""
  if wait $pid && /usr/bin/grep -aq '^ui-script: ok' "$tmp/together.log"; then
    ok "by script: a party by the page, its line and a whisper answered, $thing bought for 3 s in the trade window, an avatar hired, the party left"
    done_n=$((done_n + 1))
  else
    show "$tmp/together.log"; show "$tmp/bojan.log" 6; fail "the desktop client did not reach the end of its script"; return
  fi
  quiet "$tmp/together.log" "the desktop client, among people"
  if [[ -n "$t_party" && -n "$t_traded" ]]; then
    atmost "$(since "$t_party" "$t_traded")" "$(budget party max_secs_party_to_traded)" "seconds from being in the party to the trade being done (software GPU, by script)"
  else
    fail "the run was not timed"
  fi
  books "after the desktop client" "$((done_n * PRICE))" "$((done_n * HIRE * 70 / 100))"
}

browser() {
  command -v node >/dev/null || { fail "--browser needs node"; return; }
  [[ "${SKIP_BUILD:-}" == 1 && -f target/web/gm-client-webgpu_bg.wasm ]] || scripts/build-web.sh > target/web-build.log 2>&1 \
    || { tail -30 target/web-build.log; fail "the web build"; return; }
  local http=$((20000 + RANDOM % 20000)) hub_url hub_hash
  mkdir -p "$tmp/web"; rm -f "${tmp:?}/web"/*
  ln -s "$ROOT"/target/web/* "$tmp/web/"; rm -f "${tmp:?}/web/config.json"
  hub_url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  hub_hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$tmp/hub-web.json")"
  echo "{\"hub\": \"$hub_url\", \"hub_cert_sha256\": \"$hub_hash\", \"dev\": true}" > "$tmp/web/config.json"
  (cd "$tmp/web" && exec python3 -m http.server "$http" --bind 127.0.0.1 >/dev/null 2>&1) &
  local web=$!; pids+=("$web")
  local soft=(); [[ "$SOFTWARE" == 1 ]] && soft=(--software)
  local build name script query extra
  page() { # log, script, more arguments of web-run
    local log="$1"; query="$(node -e 'process.stdout.write(encodeURIComponent(process.argv[1]))' "$2")"; shift 2
    timeout 300 node scripts/web-run.mjs --url "http://127.0.0.1:$http/?ui-script=$query$extra" --seconds 180 \
      --login "$build@gm.test" --password "a long password" "$@" ${CHROME:+--chrome "$CHROME"} "${soft[@]}" > "$log" 2>&1 || true
    /usr/bin/grep -aq '^GM-DONE ui-script: ok' "$log"
  }
  for build in webgpu webgl; do
    name="Cvita$build"
    extra=""; [[ "$build" == webgl ]] && extra="&gl=1"
    script="$(printf '%s\n' 'wait screen new character' 'field name' "type $name" 'click blade' 'click Create' \
      'wait screen characters' "expect \"$name  blade  new\"" 'quit')"
    if page "$tmp/browser-$build-new.log" "$script" --register; then
      ok "$build: a new account by the page's form and a character called $name"
    else
      show "$tmp/browser-$build-new.log"; fail "the $build build did not make its character"; continue
    fi
    quiet "$tmp/browser-$build-new.log" "$build, making its character"
    provide "$name" || continue
    # Something for the other to offer this one: an operator's dagger.
    "${hub[@]}" --grant-item Bojan dagger core/iron,frame/oak > "$tmp/grant.log" 2>&1 || { show "$tmp/grant.log"; fail "a dagger for the one in the town"; }
    script="$(printf '%s\n' 'wait screen characters' 'click Play'; together "$name" dagger; echo quit)"
    if page "$tmp/browser-$build.log" "$script" --screenshot "$tmp/browser-$build.png"; then
      ok "$build: a party by the page, its line and a whisper answered, a dagger bought in the trade window, an avatar hired, the party left"
      done_n=$((done_n + 1))
    else
      show "$tmp/browser-$build.log"; show "$tmp/bojan.log" 6; fail "the $build build did not reach the end of its script"; continue
    fi
    if /usr/bin/grep -aq "^GM-BUILD $build" "$tmp/browser-$build.log"; then ok "$build: that build ran"; else fail "$build: another build ran"; fi
    quiet "$tmp/browser-$build.log" "$build, among people"
  done
  kill $web 2>/dev/null || true
  books "after the browsers" "$((done_n * PRICE))" "$((done_n * HIRE * 70 / 100))"
}

[[ "$DESKTOP" == 1 ]] && desktop
[[ "$BROWSER" == 1 ]] && browser
[[ -n "${KEEP:-}" ]] && echo "logs and screenshots kept in $tmp"
exit $status
