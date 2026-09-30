# gbs-anywhere

Grind-by-Sync for the Mahlkönig **E64 WS** with any espresso machine.

Grind-by-Sync (GbS) lets the E64 WS dial itself in: after each shot it
compares the extraction time with the recipe's target and adjusts its grind
setting. Normally that needs Mahlkönig's own Xenia machine, with its built-in
scale, to report the shot. gbs-anywhere takes that place: you pull the shot on your own machine, time and
weigh it with your own scale, enter the numbers on your phone, and the grinder
adjusts as usual.

> Not affiliated with Mahlkönig or Hemro.

## Run it

On any computer on the same network as the grinder (PC, NAS, Raspberry Pi),
with Docker:

```sh
docker run -d --name gbs-anywhere --restart unless-stopped \
  -p 80:80 -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
```

The `/data` volume keeps the integrations you add in the app (see
[Integrations](#integrations)).

`latest` is the newest release. Pin a version with `:0.1` (newest 0.1.x) or
`:0.1.0`, or follow `main` for the newest commit, which may not be released
yet. The app shows the version at the bottom of the page; what changed is in
[CHANGELOG.md](CHANGELOG.md).

You'll need that computer's IP address on your network (e.g. `192.168.1.20`)
for the grinder and your phone. It must stay the same, so give it a fixed IP
or a DHCP reservation in your router. Port 80 must be free and reachable from
the grinder (allow it in the firewall).

Or from source, with Rust installed:

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

1. Open `http://<that IP>/` on your phone, e.g. `http://192.168.1.20/`. The header shows
   "grinder connected" while the grinder is polling.
2. Grind with a GbS recipe.
3. When the grinder says "Press grinder rotary knob to start brewing.", press
   the knob and start the shot on your machine right away.
4. Time the shot from pump start (always use the same timer), weigh the cup,
   and enter both in the app. Tap **Send to grinder**.
5. The grinder shows its new grind setting.

Shots of 10 s or less and over 80 s are ignored by the grinder, as is an
aborted shot.

## Integrations

Entering the numbers on your phone is the default and needs no setup.
Optionally, let an *integration* report the shot instead: a vendor cloud, a
connected scale, a home-automation hub. After the knob press it waits for the
shot, then sends its time and weight to the grinder by itself. You can still
enter or correct a shot on your phone; whichever comes first wins, also when
several integrations run at once.

Tap the green **+** at the top right of the app, pick one and fill in its
form. Each
integration gets a card showing whether it is connected and what it last
sent; **Remove** stops it. Integrations added in the app are saved in the
settings file (`--config`, `/data/gbs-anywhere.json` in Docker, holding their
passwords), so keep the `/data` volume. Without a settings file they last
until the next restart.

Integrations can also be set with flags or environment variables. Those show
"Set on the command line" and can only be turned off by removing the flags.

| integration | reads | setup |
|---|---|---|
| La Marzocco cloud | time and weight from a connected La Marzocco's coffee log | [src/integration/la_marzocco](src/integration/la_marzocco/README.md) |

## Options

Extra flags go after the image name (Docker) or after `--` (cargo):

| flag | default | what |
|---|---|---|
| `--brew-timeout-s <s>` | 180 | give up on a shot with no numbers after this long (up to 3600; 0 waits forever) |
| `--config <file>` | off (Docker: `/data/gbs-anywhere.json`) | settings file for the integrations added in the app; also `CONFIG_FILE` |
| `--icons <dir>` | off | serve `<integration id>.svg/.png` from here instead of the built-in glyphs |
| `--log <file>` | off | append every grinder request to a file |
| `-p, --ports <list>` | 80 | ports to serve on |
| `--lm-*` | off | La Marzocco cloud, see [its README](src/integration/la_marzocco/README.md) |

Without Docker you can also type the shot into the console: `30 36` means
30 s, 36 g. `h` lists the other commands.

## Control API

JSON on the same port as the app, for scripts or another front end:

| method | path | what |
|---|---|---|
| GET | `/api/state` | version, phase, machine state, last shot, grinder connection, integrations |
| GET | `/api/events?after=N` | events with `seq > N` |
| GET | `/api/events/stream` | the same, live, as server-sent events |
| POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` reports the running shot (time up to 600 s; without a weight, the recipe weight is sent) |
| POST | `/api/shot/abort` | aborts the running shot (the grinder skips it) |
| GET | `/api/integrations/kinds` | the integrations that can be added, with their setup forms |
| POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{"username":"…","password":"…"}}` adds an integration |
| DELETE | `/api/integrations/<id>` | removes an integration added in the app |

The API has no login: anyone on your network who can open the app can add or
remove integrations, as they can report shots. Passwords are never sent back.

## Build

```sh
cargo build --release     # binary: target/release/gbs-anywhere
cargo test
docker build -t gbs-anywhere .
```

### Adding an integration

Each integration is one folder under `src/integration/`; `la_marzocco/` is a
complete example. The layout and the rules every integration follows are in
the [`integration` module docs](src/integration/mod.rs) (`cargo doc --open`).
The app builds its list behind **+** and the setup form from the folder's
`KIND`, so nothing in `web/` changes.

## License

Copyright 2026 Skamba and the gbs-anywhere contributors.

[GNU AGPL v3 or later](LICENSE): anyone, cafés included, may use, change and
share it. If you share a changed version, or let other people use one over a
network, you must offer them its source code under the same license.
