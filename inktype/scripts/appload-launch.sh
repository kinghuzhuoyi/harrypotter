#!/bin/sh
HERE=$(cd "$(dirname "$0")" && pwd)
systemctl is-active --quiet inktype-takeover && exit 0
systemd-run --unit=inktype-takeover --collect \
    --property="ExecStopPost=-/bin/systemctl start xochitl" \
    /bin/bash "$HERE/inktype-takeover.sh" \
  || systemd-run --unit=inktype-takeover --collect /bin/bash "$HERE/inktype-takeover.sh"
exit 0
