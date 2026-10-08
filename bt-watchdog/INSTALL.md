# Bluetooth watchdog for the Raspberry Pi

Checks Bluetooth every minute and restarts it when the controller hangs.
Raspberry PI 3b seems to have some problems with hanging bluetooth, maybe other versions, too.

## Install

```sh
sudo cp bt-watchdog.sh /usr/local/bin/
sudo chmod +x /usr/local/bin/bt-watchdog.sh
sudo cp bt-watchdog.service bt-watchdog.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now bt-watchdog.timer
```

## Check

```sh
systemctl list-timers bt-watchdog.timer   # next run
journalctl -t bt-watchdog                  # restarts and why
```

## Remove

```sh
sudo systemctl disable --now bt-watchdog.timer
sudo rm /etc/systemd/system/bt-watchdog.* /usr/local/bin/bt-watchdog.sh
sudo systemctl daemon-reload
```
