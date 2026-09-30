#!/usr/bin/env bash
# Netcode gate (PROTOCOL.md 9, PLAN.md 11.8 Phase 2): 16 bots under turmoil at 150 ms RTT and
# 3% loss must stay under budgets.toml [net] max_bytes_per_player_s in both directions and meet
# the playability assertions. Runs in simulated time (a few seconds of wall clock).
set -euo pipefail
cd "$(dirname "$0")/.."
if [ ! -f assets/maps/built/test_room.bsp ]; then
  echo "check-netcode: assets/maps/built/test_room.bsp missing; build it with gm-tools map build" >&2
  exit 1
fi
exec cargo test -q -p gm-server --locked --test netcode --test loopback -- --nocapture "$@"
