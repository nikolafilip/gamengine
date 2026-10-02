#!/usr/bin/env bash
# Build the browser client (docs/WEB.md 3.1) into target/web/: both builds of the .wasm with
# their wasm-bindgen glue, the page, its loader and the assets a browser fetches.
#   scripts/build-web.sh [--no-opt] [--config FILE]
# --config FILE copies FILE as config.json (where the hub is); the default names no hub and
# marks the site as a development one (links may then carry connect= and a login).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/target/web"
TOOLS="${WEB_TOOLS_DIR:-$ROOT/tools/web}"
OPT=1
CONFIG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-opt) OPT=0 ;;
    --config) CONFIG="$2"; shift ;;
    *) echo "build-web: unknown argument $1"; exit 2 ;;
  esac
  shift
done
"$ROOT/scripts/fetch-web-tools.sh" >/dev/null
rustup target list --installed | /usr/bin/grep -q wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown

mkdir -p "$OUT/assets/maps" "$OUT/assets/textures"
build() { # name, cargo feature flags...
  local name="$1"; shift
  cargo build -p gm-client --target wasm32-unknown-unknown --profile web "$@"
  "$TOOLS/wasm-bindgen" --target web --no-typescript --out-dir "$OUT" --out-name "gm-client-$name" \
    "$ROOT/target/wasm32-unknown-unknown/web/gm-client.wasm"
  if [[ "$OPT" == 1 ]]; then
    "$TOOLS/wasm-opt" -Oz --enable-bulk-memory --enable-bulk-memory-opt --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue --enable-call-indirect-overlong -o "$OUT/gm-client-${name}_bg.wasm" "$OUT/gm-client-${name}_bg.wasm"
  fi
}
build webgpu
build webgl --features webgl

cp "$ROOT/web/index.html" "$ROOT/web/boot.js" "$OUT/"
cp "$ROOT"/assets/maps/built/*.bsp "$ROOT"/assets/maps/built/*.lit "$OUT/assets/maps/"
cp "$ROOT/assets/textures/palette.lmp" "$OUT/assets/textures/"
if [[ -n "$CONFIG" ]]; then
  cp "$CONFIG" "$OUT/config.json"
elif [[ ! -f "$OUT/config.json" ]]; then
  echo '{"hub": null, "hub_cert_sha256": null, "dev": true}' > "$OUT/config.json"
fi

size() { stat -c %s "$1"; }
packed() { if command -v brotli >/dev/null 2>&1; then brotli -q 11 -c "$1" | wc -c; else gzip -9 -c "$1" | wc -c; fi; }
PACKER=$(command -v brotli >/dev/null 2>&1 && echo brotli || echo gzip)
for name in webgpu webgl; do
  echo "web: gm-client-$name wasm_bytes=$(size "$OUT/gm-client-${name}_bg.wasm") wasm_${PACKER}_bytes=$(packed "$OUT/gm-client-${name}_bg.wasm") js_bytes=$(size "$OUT/gm-client-$name.js")"
done
echo "web: loader_bytes=$(( $(size "$OUT/boot.js") + $(size "$OUT/index.html") )) assets_bytes=$(du -sb "$OUT/assets" | cut -f1) out=$OUT"
