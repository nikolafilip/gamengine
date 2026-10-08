#!/usr/bin/env bash
# A world to play in by hand: a private Postgres, the hub, the town, the arena and the
# tutorial dungeon, and bots that make the town worth visiting (a stall keeper, a quartermaster, three
# avatars for hire, a sociable Bojan, strollers) and the arena a fight (duelists).
# Everything lives under $GM_PLAY_DIR (default ~/.local/share/gamengine/play) and keeps
# its accounts and characters between runs.
#
#   scripts/dev/play.sh start [--fresh] [--lan [IP]]   build-free: uses target/release (cargo build --release first)
#   scripts/dev/play.sh stop
#   scripts/dev/play.sh status
#   scripts/dev/play.sh give NAME         coin and a few items for an offline character of yours
#   scripts/dev/play.sh gm EMAIL          make an account's characters game masters (G in the game: GM.md)
#   scripts/dev/play.sh people            respawn the town's people and the arena's duelists (they live 23 h)
#   scripts/dev/play.sh spar [N] [BUILDS] N sparring partners in the town (stand, fight back): tune on them
#   scripts/dev/play.sh client [ARGS...]  the windowed client on this stack (ALSA_CARD=1 for sound here)
#   scripts/dev/play.sh web               serve the browser build against this hub on http://127.0.0.1:24510
#   scripts/dev/play.sh push [USER@HOST]  the native client bundle of this build to a Linux machine's
#                                         ~/Downloads/gamengine-client over ssh (default pezo@192.168.1.41)
#
# --lan lets other machines of the network in: the hub and the zones listen on every
# interface and name this machine's address (found from the default route, or the IP given)
# to their clients, and `web` also serves the page on https://IP:24510 (a LAN address is a
# secure context only over https, and WebTransport wants one: the certificate is this
# script's own, so the browser asks once) with a native client for another Linux machine
# beside it (gamengine-client.tar.gz). Anyone on the network can then register and play.
#
# Ports 24500–24510 and a Postgres on 54330 in the play directory: nothing a gate uses
# (GM_PLAY_PORT, GM_PLAY_PG, GM_PLAY_DIR and GM_PLAY_BIN move all of it).
set -u
R="$(cd "$(dirname "$0")/../.." && pwd)"
D="${GM_PLAY_DIR:-$HOME/.local/share/gamengine/play}"
# GM_PLAY_PORT (default 24500) is the first of the eleven ports; GM_PLAY_PG the Postgres
# port; GM_PLAY_BIN the binaries: a second stack (another directory, another port base,
# debug binaries) runs beside the first without touching it.
P="${GM_PLAY_PORT:-24500}"
PG="$D/pg"; SOCK="$D/pgsock"; PGPORT="${GM_PLAY_PG:-54330}"
# A Unix socket path may have 107 bytes; a deep play directory gets its socket in /tmp.
[[ ${#SOCK} -gt 90 ]] && SOCK="/tmp/gamengine-play-$(id -u)"
URL="postgres://gm@localhost:$PGPORT/gm_play?host=$SOCK"
HUB=127.0.0.1:$P
B="${GM_PLAY_BIN:-$R/target/release}"
SECRET=play
mkdir -p "$D" "$SOCK" "$D/log"

# The stack needs the hub, the zones and the bots; the client only `client` and `push` (a
# server in the cloud has none: scripts/dev/cloud.sh).
need() { for b in gm-hub gm-server gm-bot; do [[ -x "$B/$b" ]] || { echo "missing $B/$b: run  cargo build --release  first"; exit 1; }; done; }
need_client() { [[ -x "$B/gm-client" ]] || { echo "missing $B/gm-client: run  cargo build --release  first"; exit 1; }; }
pg_up() { pg_ctl -D "$PG" status >/dev/null 2>&1; }
hub_up() { [[ -f "$D/pids" ]] && kill -0 "$(head -1 "$D/pids")" 2>/dev/null; }
psql_play() { psql -h "$SOCK" -p "$PGPORT" -U gm -d postgres "$@"; }
hubctl() { "$B/gm-hub" --database-url "$URL" "$@"; }
# The address a stack started with --lan gives its clients (nothing: this machine only).
lan() { [[ -s "$D/lan" ]] && cat "$D/lan"; }
route_ip() { ip -4 route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p'; }

start_pg() {
  if [[ ! -s "$PG/PG_VERSION" ]]; then
    initdb -D "$PG" -U gm --auth=trust >"$D/log/initdb.log" 2>&1 || { cat "$D/log/initdb.log"; exit 1; }
  fi
  pg_up || pg_ctl -D "$PG" -l "$D/log/postgres.log" -o "-k $SOCK -p $PGPORT -c listen_addresses=''" start >/dev/null || exit 1
  for _ in $(seq 1 50); do psql_play -c 'select 1' >/dev/null 2>&1 && break; sleep 0.1; done
  psql_play -tAc "select 1 from pg_database where datname='gm_play'" | /usr/bin/grep -q 1 || psql_play -c 'create database gm_play' >/dev/null
}

# A bot through the hub. The first argument is the name of its log. `spawn` backgrounds the
# program itself (not a shell function: that would record a subshell's pid and hold the caller's pipe).
BOT=("$B/gm-bot" --hub "$HUB" --hub-cert "$D/hub.der" --maps-dir "$R/assets/maps/built" --password bots-password)
bot() { local log=$1; shift; "${BOT[@]}" "$@" >"$D/log/$log.log" 2>&1; }
spawn() { local log=$1; shift; "${BOT[@]}" "$@" >"$D/log/$log.log" 2>&1 & echo $! >>"$D/bots"; }

start() {
  need; cd "$R"
  if hub_up; then echo "already running (status: $0 status)"; exit 0; fi
  local wipe=() ip="" bind=127.0.0.1 host=127.0.0.1
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --fresh) wipe=(--wipe) ;;
      --lan)
        if [[ "${2:-}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then ip=$2; shift; else ip="$(route_ip)"; fi
        [[ -n "$ip" ]] || { echo "--lan: no address found from the default route: name one (--lan IP)"; exit 1; } ;;
      *) echo "start: unknown argument $1"; exit 2 ;;
    esac
    shift
  done
  start_pg
  rm -f "$D/lan"; [[ -n "$ip" ]] && { echo "$ip" >"$D/lan"; bind=0.0.0.0; host=$ip; }
  : >"$D/pids"; : >"$D/bots"; rm -f "$D/hub.der" "$D/hub-web.json"
  # shellcheck disable=SC2086
  "$B/gm-hub" --database-url "$URL" "${wipe[@]}" --listen "$bind:$P" --cert-out "$D/hub.der" --key "$D/hub.key" \
    --zone-secret "$SECRET" --models-dir "$D/models" --start-zone town --auth-per-minute 1000 \
    --web-listen "$bind:$((P + 1))" ${ip:+--web-url "https://$ip:$((P + 1))"} --web-info-out "$D/hub-web.json" >"$D/log/hub.log" 2>&1 &
  echo $! >>"$D/pids"
  for _ in $(seq 1 100); do [[ -s "$D/hub.der" && -s "$D/hub-web.json" ]] && break; sleep 0.1; done
  [[ -s "$D/hub.der" ]] || { tail -5 "$D/log/hub.log"; echo "the hub did not come up"; stop; exit 1; }
  zone() { # name port extra...
    local name=$1 port=$2; shift 2
    # shellcheck disable=SC2086
    # One tuning file for all three zones (GM.md 3): a game master's numbers set in the
    # town are read by the arena and the dungeon at their next start.
    "$B/gm-server" --map "assets/maps/built/$name.bsp" --listen "$bind:$port" --cert-out "$D/$name.der" \
      --hub "$HUB" --hub-cert "$D/hub.der" --zone-id "$name" --zone-secret "$SECRET" --report-secs 30 \
      --web-listen "$bind:$((port + 1))" ${ip:+--public-addr "$ip:$port" --web-url "https://$ip:$((port + 1))"} \
      --tuning "$D/tuning.toml" "$@" >"$D/log/$name.log" 2>&1 &
    echo $! >>"$D/pids"
  }
  zone town $((P + 2)) --hz 20
  zone arena $((P + 4)) --replay-dir "$D/replays"
  zone dungeon $((P + 6)) --squads --recruits ironclad,mender,frostweaver --arrive-at-entry
  sleep 1.5
  printf 'hub = "%s"\nhub_cert = "%s"\n' "$host:$P" "$D/hub.der" >"$D/settings.toml"
  people
  cat <<EOF
hub $host:$P (cert $D/hub.der); zones town (20 Hz), arena, dungeon; logs in $D/log
settings: $D/settings.toml

  play:     $0 client                    (first run: "New account", a name, Create, Play)
  coin:     $0 give NAME                 (after the character exists and is logged out)
  browser:  $0 web                       then open http://127.0.0.1:$((P + 10))/
  stop:     $0 stop
EOF
  [[ -z "$ip" ]] || echo "  network:  $0 web                       then, on another machine, https://$ip:24510/"
}

# A bot's day: a session token lives 24 h (SESSION_SECS), so a bot that plays longer ends
# with "hub refused: unauthorized" at its leaving save. Bots are respawned instead (`people`).
BOT_SECS=82800

# The town's people, on a running stack (start calls this; `people` again respawns whoever
# has gone: the bots live BOT_SECS and a stack runs for days). Registering is idempotent
# (--register on an existing account logs in). Three avatars for hire in the tavern: their
# owners list them and stay offline.
people() {
  need; cd "$R"; hub_up || { echo "start the stack first"; exit 1; }
  bot owners --bots 3 --user 'owner-{i}@bots.test' --register --character 'Avatar{i}' \
    --builds ironclad,mender,frostweaver --zone town --list-for-hire 150 --secs 0
  # A stall keeper: walks to the first market tile, opens its stall, and lists whatever an
  # operator hands it at 1 s 20 c: every weapon of items.toml, one plain and one fine of
  # each, twelve for the stall's twelve slots. Handed once: a keeper that still carries
  # anything (its stall is open, or the stall closed and its stock came back) is not handed
  # more (--fresh forgets everything).
  spawn keeper --bots 1 --user keeper@bots.test --register --character Keeper --builds ironclad --zone town \
    --stalls 1 --sell-at 120 --secs $BOT_SECS --behaviour stroll
  for _ in $(seq 1 100); do /usr/bin/grep -aq "stall stands" "$D/log/keeper.log" 2>/dev/null && break; sleep 0.2; done
  if [[ "$(psql "$URL" -Atc "select count(*) from items i join holders h on h.id = i.holder_id join characters c on c.id = h.character_id \
      or c.id = (select owner_character from stalls s where s.holder_id = h.id) where c.name = 'Keeper'" 2>/dev/null || echo 0)" == 0 ]]; then
    for item in "sword core/iron,frame/oak" "sword core/dragonbone,frame/whalebone,catalyst/ember,gem/opal" \
                "dagger core/iron,frame/oak" "dagger core/dragonbone,frame/ash,catalyst/umbra,gem/opal" \
                "hammer core/iron,frame/oak" "hammer core/dragonbone,frame/whalebone,catalyst/basalt,gem/opal" \
                "staff core/iron,frame/ash" "staff core/dragonbone,frame/whalebone,catalyst/rime,gem/opal" \
                "crossbow core/iron,frame/oak" "crossbow core/dragonbone,frame/whalebone,gem/opal" \
                "musket core/iron,frame/oak" "musket core/dragonbone,frame/whalebone,gem/opal"; do
      # shellcheck disable=SC2086
      hubctl --grant-item Keeper $item >>"$D/log/grants.log" 2>&1
    done
  fi
  # The quartermaster (MODES.md 11.4): a second stall on the square, stocked with stacks
  # of rounds and kits at 1 s each, three stacks of each kind for its twelve slots. The
  # buyer's stack takes them up to its cap (30 balls, 64 pistol rounds, 90 carbine rounds,
  # 5 kits); past the cap the buy is refused in words. Handed again whenever it carries
  # nothing (sold out, or the stall closed and its stock came back and went). Handed in
  # three rounds, one stack of each kind a round, waiting for the bot to list them: two
  # stacks of one kind in one inventory merge into one (the cap is the buyer's too).
  spawn quartermaster --bots 1 --user quartermaster@bots.test --register --character Quartermaster \
    --builds ironclad --zone town --stalls 1 --sell-at 100 --secs $BOT_SECS --behaviour stroll
  for _ in $(seq 1 100); do /usr/bin/grep -aq "stall stands" "$D/log/quartermaster.log" 2>/dev/null && break; sleep 0.2; done
  local qm_items="select count(*) from items i join holders h on h.id = i.holder_id join characters c on c.id = h.character_id \
      or c.id = (select owner_character from stalls s where s.holder_id = h.id) where c.name = 'Quartermaster'"
  local qm_carried="select count(*) from items i join holders h on h.id = i.holder_id join characters c on c.id = h.character_id \
      where h.kind = 'character' and c.name = 'Quartermaster'"
  if [[ "$(psql "$URL" -Atc "$qm_items" 2>/dev/null || echo 0)" == 0 ]]; then
    for _ in 1 2 3; do
      for stack in "ball 10" "pistol_round 16" "carbine_round 30" "kit 1"; do
        # shellcheck disable=SC2086
        hubctl --grant-item Quartermaster $stack >>"$D/log/grants.log" 2>&1
      done
      for _ in $(seq 1 50); do [[ "$(psql "$URL" -Atc "$qm_carried" 2>/dev/null || echo 1)" == 0 ]] && break; sleep 0.2; done
    done
  fi
  # Bojan: joins a party when asked (P → Invite), answers a party line and a whisper, trades.
  spawn bojan --bots 1 --user bojan@bots.test --register --character Bojan --builds blade --zone town \
    --secs $BOT_SECS --behaviour hold --sociable --trade-for 300
  # Four strollers on the square, and eight duelists keeping the arena warm.
  spawn walkers --bots 4 --user 'walker-{i}@bots.test' --register --character 'Walker{i}' \
    --builds ironclad,blade,frostweaver,shade --zone town --secs $BOT_SECS --behaviour stroll
  spawn duelists --bots 8 --user 'duelist-{i}@bots.test' --register --character 'Duelist{i}' \
    --builds ironclad,blade,frostweaver,shade --zone arena --secs $BOT_SECS --behaviour duelist
}

# Sparring partners in the town: they stand where they arrive, face whoever comes at them
# and fight back with the whole kit, so a windup, a cast or a parry can be watched and
# tuned on a body that answers. `spar [N] [BUILDS]`: N of them (default 3), one build each
# from the list (default a sword, a crossbow, a caster with Ice shard).
spar() {
  need; cd "$R"; hub_up || { echo "start the stack first"; exit 1; }
  local n=${1:-3} builds=${2:-blade,marksman,frostweaver}
  spawn spar --bots "$n" --user 'spar-{i}@bots.test' --register --character 'Spar{i}' \
    --builds "$builds" --zone town --secs $BOT_SECS --behaviour spar
  echo "$n sparring partners on their way to the town (log: $D/log/spar.log)"
}

web_down() {
  [[ -f "$D/web.pid" ]] || return 0
  while read -r p; do kill "$p" 2>/dev/null; done <"$D/web.pid"
  rm -f "$D/web.pid"
}

stop() {
  # Bots first (TERM: a program started in the background by a script ignores INT unless it
  # installs a handler; the bots do not), then the zones and the hub (INT: they save).
  [[ -f "$D/bots" ]] && { while read -r p; do kill -TERM "$p" 2>/dev/null; done <"$D/bots"; rm -f "$D/bots"; sleep 1; }
  if [[ -f "$D/pids" ]]; then
    tac "$D/pids" | while read -r p; do kill -INT "$p" 2>/dev/null; done
    for _ in $(seq 1 50); do alive=0; while read -r p; do kill -0 "$p" 2>/dev/null && alive=1; done <"$D/pids"; [[ $alive == 0 ]] && break; sleep 0.1; done
    while read -r p; do kill -0 "$p" 2>/dev/null && kill -TERM "$p" 2>/dev/null; done <"$D/pids"
    rm -f "$D/pids"
  fi
  web_down
  pg_up && pg_ctl -D "$PG" stop -m fast >/dev/null
  echo "stopped"
}

status() {
  pg_up && echo "postgres: up ($PGPORT)" || echo "postgres: down"
  if hub_up; then
    echo "hub: up ($HUB)$(lan >/dev/null && echo ", open to the network as $(lan)")"
    for z in town arena dungeon; do
      printf '%-8s %s\n' "$z" "$(/usr/bin/grep -a 'zone report' "$D/log/$z.log" | tail -1 | sed 's/\x1b\[[0-9;]*m//g; s/.*zone report //')"
    done
    hubctl --audit 2>/dev/null | tail -1
  else echo "hub: down"; fi
}

give() {
  local who=${1:?the name of a character}
  hubctl --grant-coin "$who" 12345 && hubctl --grant-item "$who" sword core/iron,frame/oak \
    && hubctl --grant-item "$who" cuirass core/iron,frame/oak && hubctl --grant-item "$who" dagger core/iron,frame/oak
}

# A game master (GM.md 1): the hub's moderator flag on the account; its characters get the
# page at their next entry into a zone.
gm() {
  local email=${1:?the email of the account}
  hubctl --grant-moderator "$email" && echo "$email: a game master from the next zone entered (G opens the page)"
}

client() { need_client; cd "$R" && exec env ALSA_CARD="${ALSA_CARD:-1}" "$B/gm-client" --settings "$D/settings.toml" "$@"; }

# The page's server: Python's, with every file told "no-cache" (revalidate: 304 when the
# file is the same), so a browser never plays last build's client against this build's zone
# (WEB.md 5). With a certificate and key it serves https (the LAN).
WEB_SERVER='
import http.server, ssl, sys
class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cache-Control", "no-cache")
        super().end_headers()
server = http.server.ThreadingHTTPServer((sys.argv[1], int(sys.argv[2])), Handler)
if len(sys.argv) > 3:
    tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls.load_cert_chain(sys.argv[3], sys.argv[4])
    server.socket = tls.wrap_socket(server.socket, server_side=True)
server.serve_forever()
'

web() {
  [[ -s "$R/target/web/index.html" ]] || { echo "no browser build: scripts/build-web.sh first"; exit 1; }
  hub_up || { echo "start the stack first"; exit 1; }
  web_down
  rm -rf "${D:?}/web"; mkdir -p "$D/web"; ln -s "$R"/target/web/* "$D/web/"; rm -f "$D/web/config.json"
  local url hash
  url="$(sed -n 's/.*"url": "\([^"]*\)".*/\1/p' "$D/hub-web.json")"
  hash="$(sed -n 's/.*"cert_sha256": "\([^"]*\)".*/\1/p' "$D/hub-web.json")"
  echo "{\"hub\": \"$url\", \"hub_cert_sha256\": \"$hash\", \"dev\": true}" >"$D/web/config.json"
  (cd "$D/web" && exec python3 -c "$WEB_SERVER" 127.0.0.1 $((P + 10)) >"$D/log/http.log" 2>&1) &
  echo $! >"$D/web.pid"
  echo "http://127.0.0.1:$((P + 10))/  (the page's own form logs in; the pinned hub cert lives 13 days)"
  local ip; ip="$(lan)" || return 0
  # The page's certificate: kept while it names this address and has a day left, so a
  # browser that accepted it is not asked again at every start.
  local crt="$D/web-cert.pem" key="$D/web-key.pem"
  if ! openssl x509 -in "$crt" -noout -checkend 86400 -ext subjectAltName 2>/dev/null | /usr/bin/grep -q "IP Address:$ip\$"; then
    openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=gamengine play" \
      -addext "subjectAltName=IP:$ip" -keyout "$key" -out "$crt" 2>>"$D/log/https.log" \
      || { echo "openssl made no certificate ($D/log/https.log)"; exit 1; }
  fi
  # The native client for another Linux machine, and the hub's certificate it pins (a new
  # one at every start of the hub: the launcher fetches it each time).
  rm -rf "${D:?}/bundle" "$D/web/gamengine-client.tar.gz"
  if [[ -x "$B/gm-client" ]]; then
  mkdir -p "$D/bundle/gamengine-client"
  ln -s "$B/gm-client" "$R/assets" "$D/bundle/gamengine-client/"
  cat >"$D/bundle/gamengine-client/play" <<EOF
#!/usr/bin/env bash
# The gamengine client for the play stack at $ip (made by scripts/dev/play.sh web).
cd "\$(dirname "\$0")" || exit 1
curl -fsk "https://$ip:24510/hub.der" -o hub.der || { echo "the play stack at $ip does not answer"; exit 1; }
exec ./gm-client --hub "$ip:24500" --hub-cert hub.der "\$@"
EOF
  chmod +x "$D/bundle/gamengine-client/play"
  tar -czhf "$D/web/gamengine-client.tar.gz" -C "$D/bundle" gamengine-client
  fi
  ln -s "$D/hub.der" "$D/web/hub.der"
  (cd "$D/web" && exec python3 -c "$WEB_SERVER" "$ip" 24510 "$crt" "$key" >>"$D/log/https.log" 2>&1) &
  echo $! >>"$D/web.pid"
  cat <<EOF
on another machine of the network:
  browser:  https://$ip:24510/   (accept the certificate warning once: it is this script's own)
  native:   curl -k https://$ip:24510/gamengine-client.tar.gz | tar xz && gamengine-client/play   (Linux x86-64)
EOF
}

# The current build's native client to another Linux machine over ssh, unpacked into its
# ~/Downloads/gamengine-client (after `web`, which makes the bundle; repeat after each build).
push() {
  local to="${1:-${GM_PLAY_PUSH:-pezo@192.168.1.41}}"
  [[ -d "$D/bundle/gamengine-client" ]] || { echo "no bundle: run  $0 web  first"; exit 1; }
  need; need_client
  tar -czhf "$D/web/gamengine-client.tar.gz" -C "$D/bundle" gamengine-client || exit 1
  scp -q "$D/web/gamengine-client.tar.gz" "$to:Downloads/" || exit 1
  ssh "$to" 'cd ~/Downloads && rm -rf gamengine-client && tar xzf gamengine-client.tar.gz && ls -l gamengine-client/gm-client' || exit 1
  echo "on $to:  ~/Downloads/gamengine-client/play"
}

case "${1:-}" in
  start) shift; start "$@" ;;
  stop) stop ;;
  status) status ;;
  give) shift; give "$@" ;;
  gm) shift; gm "$@" ;;
  people) people ;;
  spar) shift; spar "$@" ;;
  client) shift; client "$@" ;;
  web) web ;;
  push) shift; push "$@" ;;
  *) sed -n '2,18p' "$0"; exit 2 ;;
esac
