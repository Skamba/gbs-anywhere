# Half Decent Scale

The Half Decent Scale from Decent Espresso can share its readings over
WiFi. gbs-anywhere can read them and report each shot by itself: the time
from the first drops in the cup to the flow stopping, and the weight in the
cup. Nothing needs to be typed, and the Decent app keeps working alongside.

## Set it up

1. The scale needs firmware 3.0 or newer. Turn WiFi on: hold **O** and
   **▢** together until "HDS Setup" shows, then **Setup → Connections →
   WiFi** (older firmware: **WiFi settings → WiFi on/off**). Exit and
   restart the scale.
2. With no WiFi saved, the scale opens its own network `DecentScale`
   (password `12345678`). Join it, open `http://192.168.1.1` and enter your
   home WiFi (2.4 GHz, like the grinder).
3. Find the scale's IP address in your router and give it a DHCP
   reservation, as for the computer running gbs-anywhere.
4. In the app: the green **+** at the top right → **Half Decent Scale**,
   and enter that address, e.g. `192.168.1.30`. Outside Docker the scale's
   own name `hds.local` usually works too; inside Docker, use the IP.

Or on the command line / in Docker:

```sh
docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 \
  -e HDS_HOST=192.168.1.30 ghcr.io/skamba/gbs-anywhere
```

| setting | flag | env | default |
|---|---|---|---|
| Scale address | `--hds-host` | `HDS_HOST` | off (`hds.local` when added in the app and left empty) |

## How it works

After the knob press, gbs-anywhere tares the scale, waits for a steady
weight, then for the first drops. The shot ends when the weight stops rising; the
time from the first drops to that moment and the coffee in the cup go to
the grinder a few seconds later.

- **Times start at the first drops, not at pump start.** They come out
  shorter than a timer started with the pump, by the time before coffee
  reaches the cup. Set your recipes' target times to match.
- **Put the cup on before the coffee comes.** Placing or lifting the cup is
  recognised and not counted as coffee; press the knob and start the shot
  before the first drops reach the cup.
- **Lifting the cup ends the shot** at the last moment coffee was flowing.
- **The scale is tared at the knob press,** so its display counts the
  coffee from zero. Put the cup on before pressing the knob. Avoid taring
  during the shot.
- **Only after a knob press.** Weighing at other times is never reported.
- **The phone still works.** You can still enter or correct a shot on your
  phone; whichever comes first wins.
- Up to four apps can read the scale at once. If the scale sleeps or loses
  WiFi, gbs-anywhere reconnects by itself.

## Official logo

The app shows a generic scale glyph, because vendor logos are trademarks
and are not shipped. To show your own, put a monochrome `half_decent.svg`
or `.png` (transparent background) you are allowed to use in a directory
and pass `-v /that/dir:/icons:ro -e ICONS_DIR=/icons`; the app tints it to
match.
