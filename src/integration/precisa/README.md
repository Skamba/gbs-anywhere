# Eureka Precisa

The Eureka Precisa scale sends its weight and timer over Bluetooth.
gbs-anywhere can keep it connected and report each shot by itself: after
the knob press it tares the scale, starts its timer, watches the cup fill
and sends the time and the weight in the cup to the grinder. Nothing needs
to be typed.

It needs the image with Bluetooth in it (`latest-bluetooth`,
`nightly-bluetooth`) on a Linux computer within reach of the scale, such as
a Raspberry Pi next to the machine, with BlueZ running. See
[Bluetooth](../../../README.md#bluetooth) for what the container needs.
Docker Desktop on macOS and Windows has no Bluetooth.

## Set it up

1. Switch the scale on, and make sure no other app on a phone or tablet is
   connected to it: the scale takes one connection at a time.
2. In the app: Status → Machine integrations → green **+** → **Eureka
   Precisa**. Every field is optional; left empty, gbs-anywhere connects to
   the first scale whose name starts with `CFS-9002`, as the Precisa
   advertises itself.
3. With more than one scale in range, enter its Bluetooth address. Run the
   image with `--bluetooth-scan` to list the devices nearby with their
   names and addresses.

The `-bluetooth` image only makes the integration available; like every
integration, it does nothing until it is added. Instead of the app, it can
also be turned on with a flag or environment variable, such as
`-e PRECISA=true` in `docker run`, with the settings below.

| setting | flag | env | default |
|---|---|---|---|
| turn it on | `--precisa` | `PRECISA` | off |
| Bluetooth address | `--precisa-address` | `PRECISA_ADDRESS` | the first scale with the name below |
| Bluetooth name | `--precisa-name` | `PRECISA_NAME` | `CFS-9002` |
| Seconds without a rise | `--precisa-stable-s` | `PRECISA_STABLE_S` | 3 (1 to 15) |
| Minimum grams | `--precisa-min-g` | `PRECISA_MIN_G` | 5 |
| Minimum seconds | `--precisa-min-time-s` | `PRECISA_MIN_TIME_S` | 20 (0 to 200) |
| Milliseconds to start the machine | `--precisa-start-delay-ms` | `PRECISA_START_DELAY_MS` | 2000 (0 to 10000) |
| Reconnect pause (ms) | `--precisa-reconnect-ms` | `PRECISA_RECONNECT_MS` | 500 (500 to 3000) |
| leave the scale's timer alone | `--precisa-no-timer` | `PRECISA_NO_TIMER` | off |
| no beeps | `--precisa-no-beep` | `PRECISA_NO_BEEP` | off |
| time the whole shot, not up to the recipe weight | `--precisa-full-shot` | `PRECISA_FULL_SHOT` | off |

The last three are only on the command line. An integration set up with
any of them can't be moved into the app.

## How it works

Put the cup on the scale before pressing the knob. After the press,
gbs-anywhere waits the start delay (2 s unless set), time to start your
machine, then tares the scale and starts its timer. The shot is timed from
the end of the delay. It ends at the first of:

- **The recipe weight.** When the cup reaches the grinder's Brew Weight,
  the shot ends there, like on a Xenia: that moment's time and the weight
  go to the grinder, and the scale beeps twice, your sign to stop the
  machine. What runs on afterwards does not count. Without a recipe weight
  (or with `--precisa-full-shot`), the other two end the shot.
- **The weight stops rising** for the seconds set, with at least the
  minimum grams in the cup. The time is the moment it last rose.
- **The scale's timer is stopped**, by its button.

A shot that ends before the minimum seconds is not reported, so a pause in
the flow after pre-infusion is not taken for the end. The scale beeps four
times when nothing went to the grinder: too short, aborted, or no end seen
within two minutes. The grinder then still waits, and the shot can be typed
into the app.

- **Only after a knob press** (on an E65/E80 GbS: after a Grind-by-Sync
  grind). Weighing at other times is never reported.
- **The scale stays connected** while it is on. When it is switched off or
  out of reach, gbs-anywhere looks for it again every reconnect pause, so it
  is back soon after switching it on.
- The status line in the app shows the time and weight while a shot is
  measured.

## Official logo

The app shows a generic scale glyph, because vendor logos are trademarks
and are not shipped. To show your own, put a monochrome `precisa.svg` or
`.png` (transparent background) you are allowed to use in a directory and
pass `-v /that/dir:/icons:ro -e ICONS_DIR=/icons`; the app tints it to
match.
