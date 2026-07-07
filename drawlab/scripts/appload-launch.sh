#!/bin/sh
# AppLoad entry point for takeover mode. AppLoad runs this inside xochitl's
# world, which is about to be stopped — so detach the real launch into a
# transient systemd unit (PID-1-owned, survives xochitl) and exit immediately.
#
# Works wherever the bundle is installed: we resolve our own directory rather
# than hardcoding a path, so dropping this folder into AppLoad just works.
HERE=$(cd "$(dirname "$0")" && pwd)
systemctl is-active --quiet drawlab-takeover && exit 0
systemd-run --unit=drawlab-takeover --collect /bin/bash "$HERE/drawlab-takeover.sh"
exit 0
