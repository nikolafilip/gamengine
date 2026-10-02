#!/usr/bin/env bash
# Fetch the pinned tools of the browser build (WEB.md 3.1) into tools/web/: the wasm-bindgen
# CLI (its version must equal the wasm-bindgen crate's in Cargo.lock) and binaryen's wasm-opt.
# Neither is vendored; versions and SHA-256 are pinned here; CI runs this script.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${WEB_TOOLS_DIR:-$ROOT/tools/web}"
BINDGEN_VERSION="0.2.129"
BINARYEN_VERSION="133"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    BINDGEN_ASSET="wasm-bindgen-$BINDGEN_VERSION-x86_64-unknown-linux-musl.tar.gz"
    BINDGEN_SHA="82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e"
    BINARYEN_ASSET="binaryen-version_$BINARYEN_VERSION-x86_64-linux.tar.gz"
    BINARYEN_SHA="2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489" ;;
  *) echo "fetch-web-tools: no pinned build for $(uname -s) $(uname -m); install wasm-bindgen-cli $BINDGEN_VERSION and binaryen yourself and put wasm-bindgen and wasm-opt in $DEST"; exit 1 ;;
esac

LOCKED="$(awk '/^name = "wasm-bindgen"$/ {getline; gsub(/[^0-9.]/, ""); print; exit}' "$ROOT/Cargo.lock")"
if [[ "$LOCKED" != "$BINDGEN_VERSION" ]]; then
  echo "fetch-web-tools: Cargo.lock has wasm-bindgen $LOCKED but this script pins $BINDGEN_VERSION: update one of them"
  exit 1
fi

STAMP="bindgen-$BINDGEN_VERSION binaryen-$BINARYEN_VERSION"
if [[ -f "$DEST/.version" && "$(cat "$DEST/.version")" == "$STAMP" && -x "$DEST/wasm-bindgen" && -x "$DEST/wasm-opt" ]]; then
  echo "web tools ($STAMP) already present in $DEST"
  exit 0
fi

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
fetch() { # url sha file
  echo "fetch-web-tools: downloading $1"
  curl -sSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c - >/dev/null || { echo "fetch-web-tools: checksum mismatch for $1"; exit 1; }
}
fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$BINDGEN_VERSION/$BINDGEN_ASSET" "$BINDGEN_SHA" "$TMP/bindgen.tgz"
fetch "https://github.com/WebAssembly/binaryen/releases/download/version_$BINARYEN_VERSION/$BINARYEN_ASSET" "$BINARYEN_SHA" "$TMP/binaryen.tgz"
tar -xzf "$TMP/bindgen.tgz" -C "$TMP"
tar -xzf "$TMP/binaryen.tgz" -C "$TMP"
install -m 755 "$TMP"/wasm-bindgen-*/wasm-bindgen "$DEST/wasm-bindgen"
install -m 755 "$TMP"/binaryen-version_*/bin/wasm-opt "$DEST/wasm-opt"
echo "$STAMP" > "$DEST/.version"
echo "fetch-web-tools: installed $("$DEST/wasm-bindgen" --version) and $("$DEST/wasm-opt" --version) into $DEST"
