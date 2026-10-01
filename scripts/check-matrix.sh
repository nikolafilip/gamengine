#!/usr/bin/env bash
# Matrix gate (MATRIX.md 11, PLAN.md 11.8 Phase 3): eight ironclads against eight blades must be
# decided for the ironclads, the blades re-specced to frostweavers must beat the ironclads, and
# blades must beat frostweavers (the cycle closes). A mirror match must be even. Runs the bot
# brains straight through the simulation on the arena map; a few seconds of wall clock.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ ! -f assets/maps/built/arena.bsp ]; then
  echo "check-matrix: assets/maps/built/arena.bsp missing; build it with gm-tools map build" >&2
  exit 1
fi
exec cargo test -q -p gm-bot --release --locked --test arena -- --nocapture "$@"
