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
| Milliseconds to start the machine | `--precisa-start-delay-ms` | `PRECISA_START_DELAY_MS` | 3000 |
| Live display refresh (ms) | `--precisa-live-ms` | `PRECISA_LIVE_MS` | 250 |

## How it works

Put the cup on the scale, grind, press the knob and start the machine within
the "milliseconds to start the machine" (3000 ms by default; the app counts
down).
Then gbs-anywhere tares the scale and resets and starts its timer; the shot
is timed from that moment. With 0 it starts right at the knob press.

- **The end of the shot.** Stopping the timer on the scale ends it at once.
  Otherwise it ends when the weight has not risen by 0.3 g for 3 s with at
  least 5 g in the cup; the time is then the moment the weight last rose.
- **Time from the end of the countdown.** Measured by gbs-anywhere, not by
  the scale, whose timer only counts whole seconds.
- **The first second is ignored**, while the tare settles.
- **Test without the grinder.** A brew started by hand (without a knob press)
  is measured the same way; when the scale ends it, the brew ends with the
  scale's time and weight, as if typed in the app. The grinder sees a flush.
- **Only after a knob press.** Weighing while the grinder is not waiting is
  never reported, and presses while the scale was off do not count later.
- **One app at a time.** The scale takes one Bluetooth connection; a phone
  app connected to it keeps gbs-anywhere out, and the other way round.
- **The phone still works.** You can still enter a shot on your phone;
  whichever comes first wins.
- **Live display.** While a shot runs, the app shows the scale's weight and
  time instead of the entry fields and refreshes every 250 ms (100 to 2000,
  the setting above). The scale itself sends at its own rate.
- A switched-off scale is looked for again at least every minute.

## Building it in

The integration is listed in `src/integration/mod.rs` like La Marzocco. It
needs these crates in `Cargo.toml`:

```toml
btleplug = "0.11"
uuid = "1"
futures = "0.3"
dbus = { version = "0.9", features = ["vendored"] }
```

`dbus` with `vendored` builds libdbus into the binary, so the distroless
Docker image needs nothing extra. Bluetooth in Docker needs `--net=host` and
`-v /run/dbus:/run/dbus:ro` (see above).
