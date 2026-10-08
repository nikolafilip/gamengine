#!/usr/bin/env bash
# The play stack (scripts/dev/play.sh) on a Hetzner Cloud server, for friends over the
# internet, the browser first: the page is served over https at a real name (Caddy, a
# Let's Encrypt certificate), and the hub's and the zones' WebTransport endpoints keep
# their pinned 13-day certificates (WEB.md 2.2), which a browser accepts from a page it
# trusts. The server runs play.sh as the user gm from /home/gm/gamengine, with a private
# Postgres, under a systemd unit that starts the stack and the page at boot.
#
#   scripts/dev/cloud.sh up                 create the server (cx23 in nbg1, Debian 13), point the name at it, wait for ssh
#   scripts/dev/cloud.sh deploy [--no-build] the static binaries, the browser build, the assets and play.sh to the server; restart the stack
#   scripts/dev/cloud.sh status             the server at Hetzner, and play.sh status on it
#   scripts/dev/cloud.sh logs [NAME]        tail a log of the stack (hub, town, arena, dungeon, http, keeper ...)
#   scripts/dev/cloud.sh ssh [CMD...]       a shell on the server (root)
#   scripts/dev/cloud.sh play ARGS...       play.sh on the server as gm (give NAME, gm EMAIL, people, spar, stop, start --lan)
#   scripts/dev/cloud.sh down               delete the server (the name and the tokens stay; Hetzner bills a stopped server in full)
#
# Tokens (never committed): the Cloud project's in $GM_HCLOUD_TOKEN_FILE (default
# ~/.config/gamengine/hcloud-token), the DNS zone's in $GM_DNS_TOKEN_FILE (default
# ~/.config/gamengine/dns-token); HCLOUD_TOKEN and GM_DNS_TOKEN in the environment win.
# The server is NAME ($GM_CLOUD_HOST, default game.nimbus-development.hr) in the zone
# $GM_CLOUD_ZONE (its name); SSH as root with the key named $GM_CLOUD_SSH_KEY at Hetzner.
set -u
R="$(cd "$(dirname "$0")/../.." && pwd)"
HOST="${GM_CLOUD_HOST:-game.nimbus-development.hr}"
ZONE="${GM_CLOUD_ZONE:-nimbus-development.hr}"
SERVER="${GM_CLOUD_SERVER:-gamengine-play}"
TYPE="${GM_CLOUD_TYPE:-cx23}"       # the cheapest: 2 vCPU, 4 GB, 40 GB, about 7 EUR a month, billed by the hour
LOCATION="${GM_CLOUD_LOCATION:-nbg1}"
IMAGE="${GM_CLOUD_IMAGE:-debian-13}"
SSH_KEY="${GM_CLOUD_SSH_KEY:-pezo@archlinux}"
API=https://api.hetzner.cloud/v1
S="${GM_CLOUD_SCRATCH:-${TMPDIR:-/tmp}/gamengine-cloud}"; mkdir -p "$S"

token() {
  local f="${GM_HCLOUD_TOKEN_FILE:-$HOME/.config/gamengine/hcloud-token}"
  [[ -n "${HCLOUD_TOKEN:-}" ]] && { echo "$HCLOUD_TOKEN"; return; }
  [[ -s "$f" ]] && { tr -d '\n' <"$f"; return; }
  echo "no Hetzner token: HCLOUD_TOKEN or $f" >&2; exit 1
}
dns_token() {
  local f="${GM_DNS_TOKEN_FILE:-$HOME/.config/gamengine/dns-token}"
  [[ -n "${GM_DNS_TOKEN:-}" ]] && { echo "$GM_DNS_TOKEN"; return; }
  [[ -s "$f" ]] && { tr -d '\n' <"$f"; return; }
  echo "no DNS token: GM_DNS_TOKEN or $f" >&2; exit 1
}
# api METHOD PATH [JSON]: the Cloud API with the project's token; dns_api the same with the zone's.
api() { curl -sS -X "$1" -H "Authorization: Bearer $(token)" -H "Content-Type: application/json" ${3:+-d "$3"} "$API$2"; }
dns_api() { curl -sS -X "$1" -H "Authorization: Bearer $(dns_token)" -H "Content-Type: application/json" ${3:+-d "$3"} "$API$2"; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }
# jq-less picks: a server's record by name, its address.
server_json() { api GET "/servers?name=$SERVER" | j 'json.dumps(d["servers"][0]) if d["servers"] else ""'; }
server_ip() { api GET "/servers?name=$SERVER" | j '(d["servers"][0]["public_net"]["ipv4"] or {}).get("ip","") if d["servers"] else ""'; }
ssh_root() { ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "root@$HOST" "$@"; }

# The name's A record: made or moved to the address.
point_name() {
  local ip=$1 zone_id sub="${HOST%.$ZONE}"
  zone_id="$(dns_api GET "/zones?name=$ZONE" | j 'd["zones"][0]["id"] if d["zones"] else ""')"
  [[ -n "$zone_id" ]] || { echo "the zone $ZONE is not in the DNS token's project"; exit 1; }
  local body="{\"name\":\"$sub\",\"type\":\"A\",\"ttl\":120,\"records\":[{\"value\":\"$ip\"}]}"
  if dns_api GET "/zones/$zone_id/rrsets/$sub/A" | j '"records" in d.get("rrset",{})' | /usr/bin/grep -q True; then
    dns_api POST "/zones/$zone_id/rrsets/$sub/A/actions/set_records" "{\"records\":[{\"value\":\"$ip\"}]}" >/dev/null
  else
    dns_api POST "/zones/$zone_id/rrsets" "$body" >/dev/null
  fi
  echo "$HOST -> $ip"
}

# What the server runs at its first boot: a Postgres (its own cluster off: play.sh keeps
# one), Caddy for the page, rsync for deploy, the user gm.
CLOUD_INIT=$(cat <<'EOF'
#cloud-config
package_update: true
packages: [postgresql, caddy, rsync, curl, openssl]
runcmd:
  - systemctl disable --now postgresql
  - useradd -m -s /bin/bash gm || true
  - touch /var/lib/cloud/ready
EOF
)

up() {
  if [[ -n "$(server_ip)" ]]; then echo "the server $SERVER exists: $(server_ip)"; else
    local key_id; key_id="$(api GET "/ssh_keys?name=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1]))" "$SSH_KEY")" | j 'd["ssh_keys"][0]["id"] if d["ssh_keys"] else ""')"
    [[ -n "$key_id" ]] || { echo "no ssh key named $SSH_KEY at Hetzner"; exit 1; }
    local body; body="$(python3 -c '
import json,sys
print(json.dumps({"name":sys.argv[1],"server_type":sys.argv[2],"image":sys.argv[3],"location":sys.argv[4],
  "ssh_keys":[int(sys.argv[5])],"labels":{"app":"gamengine","env":"play"},"user_data":sys.stdin.read(),
  "public_net":{"enable_ipv4":True,"enable_ipv6":True}}))' "$SERVER" "$TYPE" "$IMAGE" "$LOCATION" "$key_id" <<<"$CLOUD_INIT")"
    local out; out="$(api POST /servers "$body")"
    echo "$out" | /usr/bin/grep -q '"server"' || { echo "$out"; exit 1; }
    echo "created $SERVER ($TYPE, $LOCATION, $IMAGE): $(echo "$out" | j 'd["server"]["public_net"]["ipv4"]["ip"]')"
  fi
  local ip; ip="$(server_ip)"
  point_name "$ip"
  ssh-keygen -R "$HOST" >/dev/null 2>&1; ssh-keygen -R "$ip" >/dev/null 2>&1
  echo -n "waiting for ssh and the first boot "
  for _ in $(seq 1 120); do
    ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=5 "root@$ip" test -f /var/lib/cloud/ready 2>/dev/null && { echo " ready"; break; }
    echo -n .; sleep 5
  done
  # The name may lag the record: ssh by the address until it resolves.
  for _ in $(seq 1 60); do getent hosts "$HOST" | /usr/bin/grep -q "$ip" && break; sleep 5; done
  echo "next:  $0 deploy"
}

# The stack's unit and the page's site, written on the server by deploy.
UNIT=$(cat <<'EOF'
[Unit]
Description=gamengine play stack (scripts/dev/play.sh)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
User=gm
WorkingDirectory=/home/gm/gamengine
Environment=PATH=/usr/lib/postgresql/17/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=/home/gm/gamengine/scripts/dev/play.sh start --lan
ExecStart=/home/gm/gamengine/scripts/dev/play.sh web
ExecStop=/home/gm/gamengine/scripts/dev/play.sh stop
TimeoutStartSec=300
TimeoutStopSec=60

[Install]
WantedBy=multi-user.target
EOF
)

deploy() {
  local build=1; [[ "${1:-}" == --no-build ]] && build=0
  local ip; ip="$(server_ip)"; [[ -n "$ip" ]] || { echo "no server: $0 up first"; exit 1; }
  if [[ $build == 1 ]]; then
    echo "== the static binaries (x86_64-unknown-linux-musl) =="
    CC_x86_64_unknown_linux_musl="${CC_x86_64_unknown_linux_musl:-gcc}" \
      cargo build --release --target x86_64-unknown-linux-musl -p gm-hub -p gm-server -p gm-bot || exit 1
    echo "== the browser build =="
    "$R/scripts/build-web.sh" || exit 1
  fi
  local M="$R/target/x86_64-unknown-linux-musl/release"
  for b in gm-hub gm-server gm-bot; do [[ -x "$M/$b" ]] || { echo "missing $M/$b: deploy without --no-build"; exit 1; }; done
  [[ -s "$R/target/web/index.html" ]] || { echo "no browser build: scripts/build-web.sh"; exit 1; }
  echo "== the tree to $HOST =="
  local T="$S/tree"; rm -rf "$T"; mkdir -p "$T/scripts/dev" "$T/target/release" "$T/target/web"
  cp "$R/scripts/dev/play.sh" "$T/scripts/dev/"
  cp "$M"/gm-hub "$M"/gm-server "$M"/gm-bot "$T/target/release/"
  cp -r "$R/assets" "$T/assets"; rm -rf "$T/assets/maps/src" "$T"/assets/maps/built/*.prt
  for f in index.html boot.js gm-client-webgpu.js gm-client-webgpu_bg.wasm gm-client-webgl.js gm-client-webgl_bg.wasm; do cp "$R/target/web/$f" "$T/target/web/"; done
  cp -r "$R/target/web/assets" "$T/target/web/assets"
  rsync -az --delete -e "ssh -o StrictHostKeyChecking=accept-new" "$T/" "root@$ip:/home/gm/gamengine/" || exit 1
  echo "== the unit, the site, the restart =="
  ssh_root bash -s <<EOF
set -e
chown -R gm:gm /home/gm/gamengine
cat >/etc/systemd/system/gamengine-play.service <<'UNIT'
$UNIT
UNIT
cat >/etc/caddy/Caddyfile <<'CADDY'
$HOST {
    reverse_proxy 127.0.0.1:24510
}
CADDY
systemctl daemon-reload
systemctl enable -q gamengine-play
systemctl restart gamengine-play
systemctl reload-or-restart caddy
sleep 3
systemctl --no-pager --lines=3 status gamengine-play | sed -n '1,3p;/Active/p'
runuser -u gm -- env PATH=/usr/lib/postgresql/17/bin:\$PATH /home/gm/gamengine/scripts/dev/play.sh status
EOF
  echo
  echo "play:  https://$HOST/   (the page's form registers and logs in; a stack's hub certificate lives 13 days: deploy, or  $0 play stop; $0 play start --lan; $0 play web  before then)"
}

status() {
  local sj; sj="$(server_json)"
  [[ -n "$sj" ]] || { echo "no server $SERVER"; return; }
  echo "$sj" | j '"%s: %s %s %s %s" % (d["name"], d["status"], d["server_type"]["name"], d["datacenter"]["location"]["name"], d["public_net"]["ipv4"]["ip"])'
  ssh_root "systemctl is-active gamengine-play caddy | paste -sd' '; runuser -u gm -- env PATH=/usr/lib/postgresql/17/bin:\$PATH /home/gm/gamengine/scripts/dev/play.sh status" 2>/dev/null
  curl -sS -o /dev/null -w "page https://$HOST/: HTTP %{http_code}\n" --max-time 10 "https://$HOST/" || true
}

logs() { local n=${1:-hub}; ssh_root "tail -n 40 /home/gm/.local/share/gamengine/play/log/$n.log"; }
play() { ssh_root "runuser -u gm -- env PATH=/usr/lib/postgresql/17/bin:\$PATH /home/gm/gamengine/scripts/dev/play.sh $*"; }

down() {
  local sj; sj="$(server_json)"; [[ -n "$sj" ]] || { echo "no server $SERVER"; return; }
  local id; id="$(echo "$sj" | j 'd["id"]')"
  ssh_root "systemctl stop gamengine-play" 2>/dev/null || true
  api DELETE "/servers/$id" | /usr/bin/grep -q '"action"' && echo "deleted $SERVER ($id); the name $HOST still points at its old address"
}

case "${1:-}" in
  up) up ;;
  deploy) shift; deploy "$@" ;;
  status) status ;;
  logs) shift; logs "$@" ;;
  ssh) shift; ssh_root "$@" ;;
  play) shift; play "$@" ;;
  down) down ;;
  *) sed -n '2,24p' "$0"; exit 2 ;;
esac
