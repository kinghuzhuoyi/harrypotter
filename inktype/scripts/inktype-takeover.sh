#!/bin/bash
restore() {
    rm -f /tmp/epframebuffer.lock
    systemctl start xochitl
}
if [ -z "${REMAGIC_SESSION:-}" ]; then
    trap restore EXIT INT TERM
    systemctl stop xochitl
fi
HERE=$(cd "$(dirname "$0")" && pwd)
if [ -f "$HERE/oracle.env" ]; then
    set -a; . "$HERE/oracle.env"; set +a
elif [ -f "$(dirname "$HERE")/riddle/oracle.env" ]; then
    # Reuse an existing Diary setup for a zero-configuration prototype run.
    set -a; . "$(dirname "$HERE")/riddle/oracle.env"; set +a
fi
rm -f /tmp/epframebuffer.lock
[ -z "${REMAGIC_SESSION:-}" ] && sleep 1

# The optional on-tablet rat install provides a persistent Python namespace
# for circled `run` commands. Starting an already-running kernel is cheap.
if [ -x /home/root/.local/bin/rat ] && [ -x /home/root/inktype-repl/.venv/bin/python ]; then
    export HOME=/home/root
    export PATH="/home/root/inktype-repl/.venv/bin:/home/root/.local/bin:/home/root/.local/node/bin:$PATH"
    # pi's built-in Google provider can reuse the same Gemini key as InkType.
    export GEMINI_API_KEY="${GEMINI_API_KEY:-${INKTYPE_OPENAI_KEY:-${RIDDLE_OPENAI_KEY:-}}}"
    mkdir -p /home/root/inktype-repl
    (cd /home/root/inktype-repl && rat start py >>/tmp/inktype-rat.log 2>&1) || true
    export INKTYPE_RAT_URL="${INKTYPE_RAT_URL:-http://127.0.0.1:8717/mcp}"
fi

cd "$HERE"
LD_LIBRARY_PATH="$HERE:/home/root/quill:/usr/lib/plugins/scenegraph" \
    HOME=/home/root "$HERE/inktype"
echo "inktype-takeover: closed ($?), restoring xochitl"
