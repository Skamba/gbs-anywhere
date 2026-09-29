# Eureka Precisa

The Eureka Precisa (a Krell CFS-9002 sold by Eureka) sends its weight and
timer over Bluetooth. With the cup on the scale, gbs-anywhere can time and
weigh every shot itself and report it the moment it ends, so nothing needs to
be typed and no machine cloud is involved.

## Set it up

The computer running gbs-anywhere needs Bluetooth (a Raspberry Pi has it; most
NAS do not, a USB dongle helps) and must be within reach of the scale. On
Linux, BlueZ must be running.

In the app: the green **+** at the top right → **Eureka Precisa**. Nothing
needs to be filled in with one scale in range.

Or on the command line / in Docker, which needs the host's Bluetooth:

```sh
docker run -d --name gbs-anywhere --restart unless-stopped --net=host \
  -v /run/dbus:/run/dbus:ro \
  -e PRECISA=true \
  ghcr.io/skamba/gbs-anywhere
```

| setting | flag | env | default |
|---|---|---|---|
| on | `--precisa` | `PRECISA` | off |
| Bluetooth address | `--precisa-address` | `PRECISA_ADDRESS` | first scale found by name |
| Bluetooth name | `--precisa-name` | `PRECISA_NAME` | `CFS-9002` |
| leave the timer alone | `--precisa-no-timer` | `PRECISA_NO_TIMER` | off |
| Seconds without a rise | `--precisa-stable-s` | `PRECISA_STABLE_S` | 3 |
| Minimum grams | `--precisa-min-g` | `PRECISA_MIN_G` | 5 |

## How it works

Put the cup on the scale, grind, press the knob and start the shot right away.
gbs-anywhere tares the scale and resets and starts its timer.

- **The end of the shot.** Stopping the timer on the scale ends it at once.
  Otherwise it ends when the weight has not risen by 0.3 g for 3 s with at
  least 5 g in the cup; the time is then the moment the weight last rose.
- **Time from the knob press.** Measured by gbs-anywhere, not by the scale,
  whose timer only counts whole seconds.
- **The first second is ignored**, while the tare settles.
- **Only after a knob press.** Weighing while the grinder is not waiting is
  never reported, and presses while the scale was off do not count later.
- **One app at a time.** The scale takes one Bluetooth connection; a phone
  app connected to it keeps gbs-anywhere out, and the other way round.
- **The phone still works.** You can still enter a shot on your phone;
  whichever comes first wins.
- A switched-off scale is looked for again at least every minute.
