#!/usr/bin/env python3
"""Record the bridge's USB console privately; optional explicit mode command."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import socket
import time

import serial

ROOT = Path(__file__).resolve().parents[1]


def open_without_reset(stream):
    # ESP-IDF monitor's no-reset sequence: both asserted at open, release RTS first.
    stream.dtr = True
    stream.rts = True
    stream.open()
    stream.rts = False
    stream.dtr = False


def computer_running(port=8089):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True
    except ConnectionRefusedError:
        return False
    except OSError as error:
        raise RuntimeError("Cannot check the desktop listener; active command not sent") from error


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", required=True, help="Board USB serial device")
    parser.add_argument("--desktop-port", type=int, default=8089)
    parser.add_argument("--seconds", type=int, default=30)
    parser.add_argument("--command", choices=("status", "mode monitor", "mode active"))
    args = parser.parse_args()
    if not 1 <= args.seconds <= 3600:
        parser.error("--seconds must be between 1 and 3600")
    if not 1 <= args.desktop_port <= 65535:
        parser.error("--desktop-port must be between 1 and 65535")
    if args.command == "mode active" and computer_running(args.desktop_port):
        parser.error("Stop the local desktop listener before activating the bridge")
    os.umask(0o077)
    destination = ROOT / ".private/logs"
    destination.mkdir(mode=0o700, parents=True, exist_ok=True)
    filename = destination / ("serial-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + ".log")
    stream = serial.Serial(port=None, baudrate=115200, timeout=0.5)
    stream.port = args.port
    open_without_reset(stream)
    captured = 0
    print(f"Recording: {filename}", flush=True)
    try:
        with filename.open("wb") as log:
            if args.command:
                stream.write(("\r" + args.command + "\r").encode())
                stream.flush()
            deadline = time.monotonic() + args.seconds
            while time.monotonic() < deadline:
                data = stream.read(stream.in_waiting or 1)
                if data:
                    log.write(data)
                    log.flush()
                    captured += len(data)
    finally:
        stream.close()
    print(json.dumps({"log": str(filename), "bytes": captured, "command": args.command}))


if __name__ == "__main__":
    main()
