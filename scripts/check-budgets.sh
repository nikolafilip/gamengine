#!/usr/bin/env bash
# Asset-budget gate (PLAN.md 2.6): every model (.glb, .gltf, ingested .gmm) and map under
# assets/ must satisfy budgets.toml. Player uploads go through gm-ingest (docs/MODELS.md 4),
# whose limits a unit test pins to the same [character] numbers.
#
#   scripts/check-budgets.sh [paths...]    check the given paths (default: assets)
#   scripts/check-budgets.sh --self-test   generate an over-budget model and prove it is rejected,
#                                          and an in-budget model and prove it is accepted
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if [[ "${1:-}" == "--self-test" ]]; then
  exec cargo run -q -p gm-tools --locked -- budget self-test
fi
exec cargo run -q -p gm-tools --locked -- budget check "${@:-assets}"
