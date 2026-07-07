#!/usr/bin/env bash
# Stage the AppLoad bundle into dist/drawlab/, ready for `remagic publish`.
# Prereq: ../quill/build.sh has produced build/drawlab and build/libquill.so.
set -euo pipefail
cd "$(dirname "$0")"

BIN=../quill/build/drawlab
[ -f "$BIN" ] || { echo "build first: ../quill/build.sh" >&2; exit 1; }
[ -f ../quill/build/libquill.so ] || { echo "missing ../quill/build/libquill.so" >&2; exit 1; }

rm -rf dist/drawlab
mkdir -p dist/drawlab
install -m 755 "$BIN" dist/drawlab/drawlab
install -m 755 ../quill/build/libquill.so dist/drawlab/
install -m 755 scripts/appload-launch.sh scripts/drawlab-takeover.sh dist/drawlab/
install -m 644 external.manifest.json icon.png dist/drawlab/

echo "staged: $(du -sh dist/drawlab | cut -f1) in dist/drawlab/"
echo "publish with: remagic publish dist/drawlab -catalog-dir <remagic checkout>"
