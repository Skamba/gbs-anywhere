# Experimental ESP32-S3 bridge

Run E65/E80 Generic WiFi feedback and La Marzocco cloud polling on a USB-powered
ESP32-S3, without leaving a computer or NAS running. The grinder calculates the
adjustment; this firmware reports the measured shot time and weight.

Not affiliated with Mahlkönig or Hemro.

This is a separate ESP-IDF C application. Its protocol and cloud authentication
are ported from [PR15](https://github.com/Skamba/gbs-anywhere/pull/15) and
[PR16 at 3b6ef9a](https://github.com/Skamba/gbs-anywhere/pull/16), with thanks to
Skamba and erikp152. It is licensed under the repository's AGPL-3.0-or-later
[license](../../LICENSE). ESP-IDF and its bundled libraries supply WiFi, TLS,
HTTP, JSON, cryptography, NVS and the USB console; no Rust runtime is required
on the board. The desktop application and Docker build are unchanged.

## Supported setup

- ESP32-S3 with **8 MB flash and 8 MB octal PSRAM**, native USB Serial/JTAG.
  Other boards and memory layouts have not been validated.
- ESP-IDF **5.5**, with its Python environment, a host C compiler and Node.js
  **18 or later** for the offline tests. Node is not needed on the board.
- One E65/E80 Generic WiFi grinder, with a reserved **IPv4 address**.
  E65W was used for the precursor's hardware test; E80 remains untested.
- One La Marzocco cloud integration. Multiple machines on an account require
  `serial` in the imported settings. Internet access and SNTP are required.
- **Measured beverage weight** must be present in the cloud coffee record.
  Missing weight is skipped; this port does not substitute recipe weight.

There is no E64 server, manual shot-entry form, Half Decent Scale integration,
new-pairing wizard, OTA update, Bluetooth or device-control page in this first
version. A computer is needed once for initial pairing, provisioning and flashing.

## One-time setup

1. Use a desktop version with PR15's E65 support to complete pairing and add
   La Marzocco cloud. For V01.36 use **8f398c5 or later**, including its three-minute
   silent period on a pairing failure. Retain the files passed to `--config` and
   `--grinder-pairing-file`. The latter must contain a saved `pairing_id` and
   `brew_device_id`. If pairing is cleared, Generic WiFi must be enabled again.
2. Stop every desktop/container/test client that contacts this grinder. Keep
   Generic WiFi enabled, select group head 1, and enable GbS on the recipe.
   The ESP32 takes over the existing identity; it must not share it with another
   active client. Stop the ESP32 before performing any future pairing procedure,
   including during the grinder's three-minute recovery period.
3. Export the grinder's certificate as PEM from its HTTPS connection using a
   trusted local connection. Verify it belongs to your grinder. Keep this file
   outside the repository; pass its path as `--grinder-cert`. It is stored in
   private NVS configuration, not compiled into the public firmware. Certificate
   trust is checked; hostname matching is skipped only for this grinder's
   self-signed certificate. The cloud uses ESP-IDF's public CA bundle and normal
   hostname verification. Redirects are disabled for both clients.
4. Activate ESP-IDF and prepare configuration (example paths and address):

   ```sh
   . "$HOME/esp/esp-idf/export.sh"
   cd firmware/esp32-s3
   python tools/provision.py \
     --cloud-config /path/to/gbs-anywhere.json \
     --pairing-file /path/to/grinder-pairing.json \
     --grinder 192.168.1.20 \
     --grinder-cert /path/to/grinder.pem
   ```

   Enter the 2.4 GHz WiFi SSID and password at the prompts. The tool imports the
   cloud credentials and derives the same installation identity as the desktop
   integration. It writes a private `.private/config-nvs.bin` and makes no network
   requests. It requires `cryptography` and `esp_idf_nvs_partition_gen` from the
   ESP-IDF environment. `--reuse-wifi` reuses the local private WiFi settings.
5. Build and test:

   ```sh
   sh tools/check.sh
   idf.py build
   ```

6. For a dedicated board whose contents you have backed up, flash the application
   and then the initial NVS image. Replace `/dev/ttyACM0` with your board's port:

   ```sh
   idf.py -p /dev/ttyACM0 flash
   python -m esptool --chip esp32s3 --port /dev/ttyACM0 \
     write_flash 0x9000 .private/config-nvs.bin
   ```

   Reset once after provisioning. Initial mode is `monitor`. Reflashing the NVS
   image resets configuration, mode and the task journal. **For later application
   updates, use `idf.py flash` only and retain NVS.** The partition layout is fixed:
   NVS `0x9000`/`0x16000`, PHY `0x1f000`, application `0x20000`/4 MB.

## Verify and take over

Read the status page at the address printed on USB, or record the USB console:

```sh
python tools/serial_log.py --port /dev/ttyACM0 --command status
```

The status page and `/api/state` are read-only and refresh every three seconds.
They report connection, memory and shot status, but never return credentials.
The page supports English, German, French, Italian, Japanese, Korean, Simplified
Chinese and Traditional Chinese. It follows the browser's language preferences;
the language selector overrides this and remembers the choice in that browser.
Numbers, units, task states and outcomes are localized. Raw diagnostic strings
and the JSON/USB interface remain unchanged. Translations are embedded in the
firmware, so there is no external translation service or runtime dependency.
Monitor mode reads the grinder/cloud without accepting tasks or reporting results.
Wait for WiFi, cloud and grinder to be connected before activating:

```sh
python tools/serial_log.py --port /dev/ttyACM0 --command 'mode active'
```

The helper checks the local desktop listener at port 8089 (`--desktop-port` can
change it). This check cannot find services on another computer, another port,
or a NAS: stopping all other clients remains your responsibility. The USB
console also accepts `status`, `mode monitor`, and `mode active` directly.
Mode is persisted. Wait for `status` to confirm the requested mode.

For each cup, grind with the GbS recipe, then pull the shot. The bridge queries
jobs about every two seconds and cloud coffee every three seconds while waiting
(60 seconds while idle; network calls can delay these intervals). After delivery,
verify the time/weight on the grinder and its adjustment. Once configured, supply
USB power from a charger; the computer is not part of the runtime path.

To return to the desktop client, set `mode monitor`, confirm it, and disconnect
power from the ESP32 before starting the desktop client. Monitor mode still sends
read requests and must not run during initial pairing/recovery.

## Delivery behavior

Only new valid cloud records after the job baseline are eligible. The newest
invalid record advances the watermark so an older valid cup cannot be substituted.
The firmware keeps a bounded 32-job journal and timestamp watermark in NVS,
records a job before accepting it, skips existing jobs at startup/takeover, and
abandons an interrupted job after a reboot. This favors skipping a cup over
replaying it. NVS failures disable further result delivery.

A relisted current job is accepted again. A newer job cancels the old one before
being taken. An unanswered brew times out after 180 seconds. Explicit
`ClientErrorOngoingJobNotExisting` replies permit reaccepting and resending the
same event. An ambiguous `brewDone` transport failure becomes `delivery_unknown`
and is not automatically replayed. These choices differ from the desktop's
recipe-weight fallback and broader result retry behavior.

## Security and validation limits

This development build has no Secure Boot or Flash Encryption. The imported
passwords and private signing key are stored in NVS; protect the board, `.private/`,
NVS images and flash backups. Private files are excluded from Git and created with
owner-only permissions on the supported POSIX host. The status page is unauthenticated
and exposes coffee/device status to the local network; keep it on a trusted LAN.

The precursor of this port completed one ESP32-S3 → La Marzocco cloud → E65W
real-shot feedback cycle with the desktop service stopped and an operator-observed
grind adjustment. That is not hardware validation of every change in this PR.
The portable certificate provisioning, translated page and error-recovery changes
need a new board run. Long-duration operation, real token expiry and all outage
scenarios remain unverified.

`tools/check.sh` runs the pure C protocol/coffee/journal tests with ASan/UBSan,
upstream installation/proof vectors, malformed configuration tests, actual NVS
image generation with synthetic credentials/certificate, and serial-helper tests.
It also checks all eight UI dictionaries, language fallback/persistence, blocked
browser storage, localized units and startup/offline states with Node's standard
library. `GBS_NODE` can select the Node executable.
It does not contact a grinder or the vendor cloud. Worker/network failure paths
are not yet covered by a full fake-service integration harness. This firmware is
an experimental contribution, with a separate build from the desktop Rust CI.
