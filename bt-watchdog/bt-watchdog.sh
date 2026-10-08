#!/bin/sh
# Restarts Bluetooth on the host when the controller hangs, as the Raspberry
# Pi's built-in one does now and then. Run every minute by bt-watchdog.timer.
#
# Restarts when: bluetoothd is not running, the controller does not answer or
# is not powered, or the kernel logged controller timeouts since the last
# check. Each restart is logged: journalctl -t bt-watchdog

set -u
TAG=bt-watchdog
STAMP=/run/bt-watchdog.last

restart() {
    logger -t "$TAG" "restarting bluetooth: $1"
    systemctl restart bluetooth
    sleep 3
    rfkill unblock bluetooth 2>/dev/null
    timeout 10 bluetoothctl power on >/dev/null 2>&1
    date +%s > "$STAMP"
    exit 0
}

systemctl is-active --quiet bluetooth || restart "service not running"

out=$(timeout 10 bluetoothctl show 2>&1) || restart "controller does not answer"
echo "$out" | grep -q "Powered: yes" || restart "controller not powered"

# Kernel messages since the last restart, at most the last 3 minutes, so old
# messages do not trigger a restart again.
since="-3min"
if [ -f "$STAMP" ]; then
    last=$(cat "$STAMP")
    [ $(( $(date +%s) - last )) -lt 180 ] && since="@$last"
fi
if journalctl -k --since "$since" --no-pager 2>/dev/null \
    | grep -Eqi "hci[0-9].*(timeout|timed out|reassembly failed|hardware error)"; then
    restart "controller timeouts in the kernel log"
fi
