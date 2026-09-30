#!/usr/bin/env bash
# Fetch the pinned ericw-tools release (qbsp, vis, light, bsputil) into tools/ericw-tools/.
# The binaries are not vendored: the Linux archive alone is 18 MB and this repository is
# measured in megabytes. The version and SHA-256 are pinned here; CI runs this script.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="2.0.0-alpha11"
DEST="${ERICW_TOOLS_DIR:-$ROOT/tools/ericw-tools}"

case "$(uname -s)" in
  Linux)  ASSET="ericw-tools-$VERSION-Linux.zip";  SHA="166109ab47657d291142992070bcb661e3029708916e152c690473dfb0f74f15" ;;
  Darwin) ASSET="ericw-tools-$VERSION-Darwin.zip"; SHA="bc5b66fb00234e3697373f38d65130fd0d14fc9eaa549277f6c93eabd4504531" ;;
  MINGW*|MSYS*|CYGWIN*) ASSET="ericw-tools-$VERSION-win64.zip"; SHA="4e5ea11be2194a1c4acac6d6da9d5b5b9f65324fda2d67efa0731d1fd8e0745f" ;;
  *) echo "fetch-ericw-tools: unsupported OS $(uname -s)"; exit 1 ;;
esac
URL="https://github.com/ericwa/ericw-tools/releases/download/$VERSION/$ASSET"

if [[ -f "$DEST/.version" && "$(cat "$DEST/.version")" == "$VERSION" && ( -e "$DEST/qbsp" || -e "$DEST/qbsp.exe" ) ]]; then
  echo "ericw-tools $VERSION already present in $DEST"
  exit 0
fi

mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
echo "fetch-ericw-tools: downloading $URL"
curl -sSL --retry 3 -o "$TMP/$ASSET" "$URL"

if [[ "$SHA" == __* ]]; then
  echo "fetch-ericw-tools: warning: no pinned checksum for $ASSET (sha256 $(sha256sum "$TMP/$ASSET" | cut -d' ' -f1))"
else
  echo "$SHA  $TMP/$ASSET" | sha256sum -c - >/dev/null || { echo "fetch-ericw-tools: checksum mismatch for $ASSET"; exit 1; }
fi

if command -v unzip >/dev/null 2>&1; then
  unzip -q -o "$TMP/$ASSET" -d "$DEST"
else
  python3 -c "import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$TMP/$ASSET" "$DEST"
fi
chmod +x "$DEST"/{qbsp,vis,light,bsputil,bspinfo,maputil,lightpreview} 2>/dev/null || true
echo "$VERSION" > "$DEST/.version"
echo "fetch-ericw-tools: installed $("$DEST/qbsp" --help 2>&1 | head -1 | tr -d '-' | xargs) into $DEST"
