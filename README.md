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
  --user 0:0 -v /run/dbus:/run/dbus:ro -v gbs-anywhere:/data \
  -e PRECISA=true \
  ghcr.io/panterro/gbs-anywhere-bt
```

| setting | flag | env | default |
|---|---|---|---|
| on | `--precisa` | `PRECISA` | off |
| Bluetooth address | `--precisa-address` | `PRECISA_ADDRESS` | first scale found by name |
| Bluetooth name | `--precisa-name` | `PRECISA_NAME` | `CFS-9002` |
| leave the timer alone | `--precisa-no-timer` | `PRECISA_NO_TIMER` | off |
| no beeps | `--precisa-no-beep` | `PRECISA_NO_BEEP` | off |
| whole shot, not to the recipe weight | `--precisa-full-shot` | `PRECISA_FULL_SHOT` | off |
| Seconds without a rise | `--precisa-stable-s` | `PRECISA_STABLE_S` | 3 |
| Minimum grams | `--precisa-min-g` | `PRECISA_MIN_G` | 5 |
| Minimum seconds | `--precisa-min-time-s` | `PRECISA_MIN_TIME_S` | 20 |
| Milliseconds to start the machine | `--precisa-start-delay-ms` | `PRECISA_START_DELAY_MS` | 2000 |
| Reconnect pause (ms) | `--precisa-reconnect-ms` | `PRECISA_RECONNECT_MS` | 500 |
| Live display refresh (ms) | `--precisa-live-ms` | `PRECISA_LIVE_MS` | 100 |

## How it works

Put the cup on the scale, grind, press the knob and start the machine within
the "milliseconds to start the machine" (2000 ms by default, 0 to 10000; the
time and the clock in the app stay at 0 until then).
Then gbs-anywhere tares the scale and resets and starts its timer; the shot
is timed from that moment. With 0 it starts right at the knob press.

- **The end of the shot.** Stopping the timer on the scale ends it at once.
  Otherwise it ends when the weight has not risen by 0.3 g for 3 s with at
  least 5 g in the cup; the time is then the moment the weight last rose.
- **Time from the end of the countdown.** Measured by gbs-anywhere, not by
  the scale, whose timer only counts whole seconds.
- **Ends at the recipe weight.** Like a Xenia, the shot ends the moment the
  cup reaches the weight the grinder's recipe asks for (Brew Weight): that
  moment is the shot's time, and the scale beeps twice right away, the sign
  to stop the machine. Whatever runs into the cup afterwards does not count,
  so stopping late does not make the shot look slow and grind coarser. Two
  readings over the target are needed, so a knock on the cup does not end
  it. Without a recipe weight, or with `--precisa-full-shot`, the shot ends
  when the flow stops, as below.
- **Minimum time.** Before "minimum seconds" (20 s by default, 0 to 200) a
  shot is not over: a pause in the flow does not end it. Stopping the scale's
  timer before then aborts it (four beeps, nothing reported); the brew keeps
  running, so it can still be entered by hand. Reaching the recipe weight
  counts even before then: a fast shot is what the grinder needs to hear.
- **The first second is ignored**, while the tare settles.
- **Beeps.** The scale beeps twice when a shot goes to the grinder (or ends a
  test), and four times when one ends without a result: aborted, answered
  in the app, or no end seen after 120 s. `--precisa-no-beep` turns this off.
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
  time instead of the entry fields and refreshes every 100 ms (50 to 2000,
  the setting above). The scale itself sends at its own rate.
- **Reconnecting.** While the scale is off or out of reach, gbs-anywhere
  searches for it over and over with the "reconnect pause" in between
  (500 ms by default, 500 to 3000), so it connects within about a second of
  switching the scale on.

## Building it in

The integration is listed in `src/integration/mod.rs` like La Marzocco. It
needs these crates in `Cargo.toml`:

```toml
btleplug = "0.11"
uuid = "1"
futures = "0.3"
```

btleplug links the system's libdbus. The Docker image therefore builds with
`libdbus-1-dev` and runs on Debian slim with `libdbus-1-3` (distroless has no
libdbus). Bluetooth in Docker needs `--net=host`, `--user 0:0` and
`-v /run/dbus:/run/dbus:ro` (see above).
