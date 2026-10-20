#!/usr/bin/env bash
# Fetch the PP-OCRv6-tiny weights used by --ocr (~7 MB, not vendored).
# Usage: scripts/fetch-models.sh [DEST]   default: ~/.local/share/tingbudont/models
set -euo pipefail

VERSION=0.3.2                       # must track the rusto-rs crate's model ABI
DEST="${1:-${XDG_DATA_HOME:-$HOME/.local/share}/tingbudont/models}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -fsSL \
  "https://registry.npmjs.org/@rustors/model-ppocrv6-tiny/-/model-ppocrv6-tiny-${VERSION}.tgz" \
  -o "$TMP/m.tgz"
tar -xzf "$TMP/m.tgz" -C "$TMP"

mkdir -p "$DEST"
cp "$TMP/package/models/det.rten" "$TMP/package/models/rec.rten" "$TMP/package/models/dict.txt" "$DEST/"
du -sh "$DEST"
