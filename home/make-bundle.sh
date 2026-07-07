#!/usr/bin/env bash
# Stage the AppLoad bundle into dist/home/, ready for `remagic publish`.
# Prereq: ../quill/build.sh has produced build/home and build/libquill.so.
set -euo pipefail
cd "$(dirname "$0")"

BIN=../quill/build/home
[ -f "$BIN" ] || { echo "build first: ../quill/build.sh" >&2; exit 1; }
[ -f ../quill/build/libquill.so ] || { echo "missing ../quill/build/libquill.so" >&2; exit 1; }

rm -rf dist/home
mkdir -p dist/home
install -m 755 "$BIN" dist/home/home
install -m 755 ../quill/build/libquill.so dist/home/
install -m 755 scripts/appload-launch.sh scripts/home-takeover.sh dist/home/
install -m 644 external.manifest.json icon.png dist/home/

echo "staged: $(du -sh dist/home | cut -f1) in dist/home/"
echo "publish with: remagic publish dist/home -catalog-dir <remagic checkout>"
