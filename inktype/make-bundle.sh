#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
BIN=target/aarch64-unknown-linux-gnu/release/inktype
[ -f "$BIN" ] || { echo "build first: ./build-takeover.sh" >&2; exit 1; }
[ -f ../quill/build/libquill.so ] || { echo "missing ../quill/build/libquill.so" >&2; exit 1; }
rm -rf dist/inktype
mkdir -p dist/inktype
install -m 755 "$BIN" dist/inktype/inktype
install -m 755 ../quill/build/libquill.so dist/inktype/
install -m 755 scripts/appload-launch.sh scripts/inktype-takeover.sh dist/inktype/
install -m 644 external.manifest.json settings.schema.json oracle.env.example icon.png dist/inktype/
echo "staged: $(du -sh dist/inktype | cut -f1) in dist/inktype/"
