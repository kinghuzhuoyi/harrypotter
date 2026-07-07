#!/bin/bash
# Launch drawlab in full-takeover mode: stop xochitl, run against the vendor
# e-ink engine (instant ink), ALWAYS restore xochitl on exit.
#
# Exit drawlab: power button, 5-finger tap, or SIGTERM. Escape hatch if
# anything wedges: ssh rm 'systemctl start xochitl'.

restore() {
    rm -f /tmp/epframebuffer.lock
    systemctl start xochitl
}
# Under the Remagic Home session host (REMAGIC_SESSION=1), xochitl is already
# stopped and the session owns its restore — skip our own stop/restart.
if [ -z "${REMAGIC_SESSION:-}" ]; then
    trap restore EXIT INT TERM
    systemctl stop xochitl
fi

# Resolve our own install directory so the bundle works wherever it lives
# (e.g. /home/root/xovi/exthome/appload/drawlab/ when installed via AppLoad).
HERE=$(cd "$(dirname "$0")" && pwd)

rm -f /tmp/epframebuffer.lock      # stale EPD lock blocks the engine
[ -z "${REMAGIC_SESSION:-}" ] && sleep 1

cd "$HERE"
# libquill.so ships in this bundle; libqsgepaper.so (reMarkable's proprietary
# engine) comes from the device's own scenegraph plugin dir. We search the
# bundle first, then a standalone /home/root/quill install, then the plugin dir.
LD_LIBRARY_PATH="$HERE:/home/root/quill:/usr/lib/plugins/scenegraph" \
    HOME=/home/root \
    "$HERE/drawlab"
echo "drawlab-takeover: closed ($?), restoring xochitl"
