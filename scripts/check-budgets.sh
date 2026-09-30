#!/usr/bin/env bash
# Asset-budget gate (PLAN.md 2.6): every model, texture and map under assets/ must satisfy
# budgets.toml. Implemented in gm-tools so the same code runs in CI and in the ingestion service.
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
