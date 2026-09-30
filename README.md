# gbs-anywhere-bt

Grind-by-Sync for the Mahlkönig **E64 WS** with any espresso machine, with a
Bluetooth scale (see integration list for models) that times and weighs the shot for you.

Grind-by-Sync (GbS) lets the E64 WS dial itself in: after each shot it
compares the extraction time with the recipe's target and adjusts its grind
setting. Normally that needs Mahlkönig's own Xenia machine, with its built-in
scale, to report the shot. gbs-anywhere takes that place: you pull the shot on
your own machine, and either enter time and weight on your phone or let a
connected scale (the Eureka Precisa) measure and send them, and the grinder
adjusts as usual.

This is a fork of [skamba/gbs-anywhere](https://github.com/skamba/gbs-anywhere)
that adds:

- the **Eureka Precisa** Bluetooth scale as an integration: it tares, starts
  its timer, sees the shot end and reports time and weight by itself;
- a **live display** in the app: the scale's weight and the shot's time
  instead of the entry fields while it measures;
- a **test mode** without the grinder;
- **Configure** on each integration's card, to change its settings in the app.

> Not affiliated with Mahlkönig, Hemro or Eureka.

## Run it

On any computer on the same network as the grinder, with Docker.

**Without a Bluetooth scale** (PC, NAS, Raspberry Pi):

```sh
docker run -d --name gbs-anywhere --restart unless-stopped -p 80:80 -v gbs-anywhere:/data ghcr.io/panterro/gbs-anywhere-bt:latest
```

**With the Eureka Precisa**, on a Linux computer with Bluetooth near the
scale, e.g. a Raspberry Pi next to the machine (BlueZ must be running):

```sh
docker run -d --name gbs-anywhere --restart unless-stopped --net=host --user 0:0 -v /run/dbus:/run/dbus:ro -v gbs-anywhere:/data ghcr.io/panterro/gbs-anywhere-bt:latest
```

`--net=host` and the D-Bus mount give the container the host's Bluetooth;
`--user 0:0` is needed because BlueZ only lets root in by default, and to
open the host's port 80. Docker Desktop on macOS and Windows has no
Bluetooth, so the scale needs Linux.

The `/data` volume keeps the integrations you add in the app (see
[Integrations](#integrations)). The app shows the version at the bottom of
the page; what changed is in [CHANGELOG.md](CHANGELOG.md).

You'll need that computer's IP address on your network (e.g. `192.168.1.20`)
for the grinder and your phone. It must stay the same, so give it a fixed IP
or a DHCP reservation in your router. Port 80 must be free and reachable from
the grinder (allow it in the firewall).

Or from source, with Rust installed (on Linux, `libdbus-1-dev` and
`pkg-config` are needed for Bluetooth):

```sh
cargo run --release
```

## Set up the E64 WS

The grinder treats gbs-anywhere as a Xenia, so the menus below say "Xenia".

**Connect the grinder**

1. Put the grinder on the same network as the computer running gbs-anywhere.
   The E64 WS only supports 2.4 GHz WiFi.
2. On the grinder, open **Settings** (lower-left button on the home screen).
3. Go to **Connectivity → Machine To Machine → Enable Xenia**.
4. Open **Configuration**. The scan will not find gbs-anywhere, so enter the
   hostname by hand: the computer's IP address, e.g. `192.168.1.20`. An IP is
   more reliable than a `.local` name.
5. Within a few seconds the app shows **grinder connected**. The grinder polls
   every 2 seconds from then on.

**Make a recipe use Grind-by-Sync**

- New recipe: **Settings → Setup Assistance → Assisted Dial-in**, follow the
  steps, save the recipe in a slot, answer **yes** to "Fine-tune with GbS?"
  and choose **GbS**.
- Existing recipe: open the recipe's menu and turn on **Enable GbS**, then set
  **Brew Time** (target time), **Brew Weight** (target weight in the cup) and
  **GbS start DD** (the grind setting to start from).

A blue chain icon on the recipe means GbS is active.

## Pull a shot

1. Open `http://<that IP>/` on your phone, e.g. `http://192.168.1.20/`. The
   header shows "grinder connected" while the grinder is polling.
2. Grind with a GbS recipe.
3. When the grinder says "Press grinder rotary knob to start brewing.", press
   the knob and start the shot on your machine.

**With the Eureka Precisa**, put the cup on the scale before pressing the
knob. The scale tares, and after a short start delay (2 s by default, time to
start the machine) its timer runs. The app shows the weight and the time
live instead of the entry fields; its clock waits out the start delay too.
When the flow stops (or you stop the scale's timer), time and weight go to
the grinder and the scale beeps twice. Four beeps mean the shot was not
reported: aborted, too short, or no end seen. **Enter by hand instead** under
the live values brings back the entry fields for that shot.

**Without a scale**, time the shot from pump start (always use the same
timer), weigh the cup, and enter both in the app. Tap **Send to grinder**.

The grinder then shows its new grind setting. Shots of 10 s or less and over
80 s are ignored by the grinder, as is an aborted shot.

### Test without the grinder

A brew can be started without the grinder: `s` in the console (without
Docker) or `POST /api/shot/start`. The grinder sees it as a flush. With the
Precisa connected, the scale measures it like a real shot and ends the brew
with its time and weight, so scale, thresholds and live display can be tried
without grinding.

## Integrations

Entering the numbers on your phone is the default and needs no setup.
Optionally, let an *integration* report the shot instead: a connected scale,
a vendor cloud, a home-automation hub. After the knob press it waits for the
shot, then sends its time and weight to the grinder by itself. You can still
enter or correct a shot on your phone; whichever comes first wins, also when
several integrations run at once.

Tap the green **+** at the top right of the app, pick one and fill in its
form. Each integration gets a card showing whether it is connected and what
it last sent; a scale's card also shows its weight live. **Configure** opens
the form again with the current settings (passwords stay as they are unless
you type a new one) and restarts the integration with the changes;
**Remove** stops it.

Integrations added in the app are saved in the settings file (`--config`,
`/data/gbs-anywhere.json` in Docker, holding their passwords), so keep the
`/data` volume. Without a settings file they last until the next restart.

Integrations can also be set with flags or environment variables. Those show
"Set on the command line" and can only be changed or turned off by changing
the flags.

| integration | reads | setup |
|---|---|---|
| Eureka Precisa | time and weight from a Eureka Precisa scale over Bluetooth, live in the app | [src/integration/eureka_precisa](src/integration/eureka_precisa/README.md) |
| La Marzocco cloud | time and weight from a connected La Marzocco's coffee log | [src/integration/la_marzocco](src/integration/la_marzocco/README.md) |

## Options

Extra flags go after the image name (Docker) or after `--` (cargo):

| flag | default | what |
|---|---|---|
| `--brew-timeout-s <s>` | 180 | give up on a shot with no numbers after this long |
| `--config <file>` | off (Docker: `/data/gbs-anywhere.json`) | settings file for the integrations added in the app; also `CONFIG_FILE` |
| `--icons <dir>` | off | serve `<integration id>.svg/.png` from here instead of the built-in glyphs |
| `--log <file>` | off | append every grinder request to a file |
| `-p, --ports <list>` | 80 | ports to serve on |
| `--precisa*` | off | Eureka Precisa, see [its README](src/integration/eureka_precisa/README.md) |
| `--lm-*` | off | La Marzocco cloud, see [its README](src/integration/la_marzocco/README.md) |

Without Docker you can also type the shot into the console: `30 36` means
30 s, 36 g. `h` lists the other commands.

## Control API

JSON on the same port as the app, for scripts or another front end:

| method | path | what |
|---|---|---|
| GET | `/api/state` | version, phase, machine state, last shot, grinder connection, integrations (with a scale's live reading) |
| GET | `/api/events?after=N` | events with `seq > N` |
| GET | `/api/events/stream` | the same, live, as server-sent events |
| POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` reports the running shot |
| POST | `/api/shot/abort` | aborts the running shot (the grinder skips it) |
| POST | `/api/shot/start` | starts a brew without the grinder (a test; the grinder sees a flush) |
| GET | `/api/integrations/kinds` | the integrations that can be added, with their setup forms |
| POST | `/api/integrations` | `{"kind":"eureka_precisa","settings":{}}` adds an integration |
| GET | `/api/integrations/<id>` | the settings of one added in the app, without passwords |
| PUT | `/api/integrations/<id>` | `{"settings":{…}}` changes them and restarts it; an empty password keeps the saved one |
| DELETE | `/api/integrations/<id>` | removes an integration added in the app |

The API has no login: anyone on your network who can open the app can add,
change or remove integrations, as they can report shots. Passwords are never
sent back.

## Build

```sh
cargo build --release     # binary: target/release/gbs-anywhere
cargo test
docker build -t gbs-anywhere-bt .
```

The Docker image runs on Debian slim rather than distroless, because the
Bluetooth stack needs the system's libdbus.

### Adding an integration

Each integration is one folder under `src/integration/`; `la_marzocco/` (a
cloud) and `eureka_precisa/` (a Bluetooth device) are complete examples. The
layout and the rules every integration follows are in the
[`integration` module docs](src/integration/mod.rs) (`cargo doc --open`).
The app builds its list behind **+** and the setup form from the folder's
`KIND`, so nothing in `web/` changes.

## License

Copyright 2026 Skamba and the gbs-anywhere contributors.

[GNU AGPL v3 or later](LICENSE): anyone, cafés included, may use, change and
share it. If you share a changed version, or let other people use one over a
network, you must offer them its source code under the same license.
