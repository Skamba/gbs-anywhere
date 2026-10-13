# gbs-anywhere

Grind-by-Sync for the Mahlkönig **E64/E65/E80** GbS grinders
(**E64WS, E65W, E65T, E80W, E80T**) with any espresso machine

Grind-by-Sync (GbS) lets the grinder dial itself in: after each shot it
compares the extraction time with the recipe's target and adjusts its grind
setting. Normally that needs a connected espresso machine to report the shot:
Mahlkönig's own Xenia for the E64 WS, or one of the few supported machines for
the E65/E80 GbS. gbs-anywhere takes that place: you pull the shot on your own
machine, time and weigh it with your own scale, enter the numbers on your
phone, and the grinder adjusts as usual.

The two grinder families connect differently:

| grinder | how it connects | setup |
|---|---|---|
| E64 WS | polls gbs-anywhere as its "Xenia" | [Set up the E64 WS](#set-up-the-e64-ws) |
| E65W, E65T, E80W, E80T GbS | gbs-anywhere pairs with the grinder's "Generic WiFi" | [Set up an E65/E80 GbS](#set-up-an-e65e80-gbs) |

Tested with the E64 WS and the E65W GbS; the other E65/E80 GbS models offer
the same "Generic WiFi" option.

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
`:0.1.0`, or follow `:nightly`, built each night from the newest commit, which
may not be released yet. The app shows the version at the bottom of the page;
what changed is in [CHANGELOG.md](CHANGELOG.md).

You'll need that computer's IP address on your network (e.g. `192.168.1.20`)
for the grinder and your phone. It must stay the same, so give it a fixed IP
or a DHCP reservation in your router. Port 80 must be free and reachable from
the grinder (allow it in the firewall).

For an E65/E80 GbS, also tell gbs-anywhere where the grinder is (see
[Set up an E65/E80 GbS](#set-up-an-e65e80-gbs)):

```sh
docker run -d --name gbs-anywhere --restart unless-stopped \
  -p 80:80 -v gbs-anywhere:/data -e GRINDER_HOST=192.168.1.30 \
  ghcr.io/skamba/gbs-anywhere
```

Or from source, with Rust installed:

```sh
cargo run --release
```

From source, pass `--config <file>` to keep integrations added in the app,
and with `--grinder` the pairing, across restarts (Docker does this in
`/data`).

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

## Set up an E65/E80 GbS

These grinders don't poll a machine. With their "Generic WiFi" option the
machine pairs with the grinder and, after every Grind-by-Sync grind, picks up
the grinder's request for that shot and answers with its time and weight.
gbs-anywhere does this when you start it with the grinder's address
(`--grinder`, or `GRINDER_HOST` in Docker, as [above](#run-it)).

The grinder's address is in its own WiFi settings, e.g. `192.168.1.30`. Give
the grinder a fixed IP or a DHCP reservation as well. gbs-anywhere talks to it
on port 30001 over HTTPS; the grinder needs no access to the computer.

**Pair the grinder**

1. Put the grinder on the same network as the computer running gbs-anywhere.
2. On the grinder, open **Settings → Connectivity → Machine To Machine** and
   enable **Generic WiFi**. Leave the other machines off. Set **Grouphead
   Selection** to **Grouphead 1** (or **All**): Grind-by-Sync only learns
   from shots of the selected group, and gbs-anywhere is a one-group machine.
3. Start gbs-anywhere with the grinder's address (`GRINDER_HOST`, see
   [Run it](#run-it)). The app now says to accept gbs-anywhere on the grinder
   and shows a pairing code.
4. On the grinder, in the same **Machine To Machine** menu, open **Generic
   WiFi → Configuration** and tap **Start Pairing**. (If you enabled Generic
   WiFi after starting gbs-anywhere, give it up to a minute to notice.) Within a few seconds **gbs-anywhere** shows up under **Available
   devices**, with the same code. Select it and tap **Accept Pairing**. If the
   grinder's pairing times out first, tap **Start Pairing** again (the code
   may change: the app always shows the current one).
5. The Generic WiFi entry is now called **gbs-anywhere**. Its **Diagnosis**
   shows the activity status as on and group 1 as idle, and the app shows
   **grinder connected**.

The pairing is kept in `/data/grinder-pairing.json` (next to the `--config`
file), so a restart needs no new pairing. Keep the `/data` volume. Deleting
that file makes gbs-anywhere pair anew; clear the old pairing on the grinder
first (**Clear Pairing**). Clear Pairing also switches Generic WiFi off:
switch it on again before you pair.

The grinder may restart right after you accept (the E65W with firmware
V01.36 does): the screen freezes, the light pulses red, then white, and the
grinder starts again. Leave gbs-anywhere running: when the grinder stops
answering while pairing, gbs-anywhere sends it nothing for 3 minutes (the
app says so), because a request while it starts up can make it lose the
pairing it has just saved. (The same happens if the grinder goes to standby
or is switched off while the pairing code is showing: then gbs-anywhere may
take up to 3 minutes to show up under **Available devices** again.) After
those 3 minutes, open **Generic WiFi → Configuration** on the grinder:

- It shows a **Pairing ID** next to the name gbs-anywhere: the pairing is
  made, but gbs-anywhere never learned its ID. The app asks for it: type the
  Pairing ID in and tap **Use this pairing ID**. gbs-anywhere checks it with
  the grinder before keeping it. (The ID is also under **Diagnosis**, after
  the serial number. `GRINDER_PAIRING_ID` does the same from the command
  line; it is only used while the saved pairing does not work.)
- It shows **Start Pairing** and no gbs-anywhere entry: the pairing was lost
  in the restart. If Generic WiFi is switched off again, enable it as in
  step 2. Then start pairing and accept once more.

If the pairing is cleared on the grinder (**Clear Pairing**), gbs-anywhere
asks to pair again by itself once Generic WiFi is switched on again.

**Make a recipe use Grind-by-Sync** as on the E64 WS: turn on **Enable GbS**
and set **Brew Time**, **Brew Weight** and **GbS start DD**.

**Pull a shot**

There is no knob press: the Grind-by-Sync grind itself tells gbs-anywhere a
shot is coming. Pull the shot, then enter its time and weight in the app
(`http://<computer's IP>/` on your phone, e.g. `http://192.168.1.20/`), or let
an [integration](#integrations) report it. gbs-anywhere passes it on to
the grinder, which adjusts as usual. gbs-anywhere keeps the grinder waiting
for the shot for up to `--brew-timeout-s` (3 minutes unless set), which leaves
time to tamp and pull; if no shot comes by then, it tells the grinder the shot
was cancelled. If you grind again before pulling the shot, the newer grind is
the one the shot counts for, and the grinder is told the older one was
cancelled. If the grinder stops waiting first (for example, it restarted), the
app shows the shot as aborted, or, once you report it, that it did not reach
the grinder. If a shot cannot reach the grinder (say, the WiFi drops),
gbs-anywhere keeps trying for two minutes and the app shows the problem.

## Pull a shot on the E64 WS

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
connected scale, a home-automation hub. After the knob press (on an E65/E80
GbS: after the grind) it waits for the shot, then sends its time and weight
to the grinder by itself. You can still
enter or correct a shot on your phone; whichever comes first wins, also when
several integrations run at once.

Tap the green **+** at the top right of the app, pick one and fill in its
form. Each
integration gets a card showing whether it is ready and what it last
sent; **Remove** stops it. Integrations added in the app are saved in the
settings file (`--config`, `/data/gbs-anywhere.json` in Docker, holding their
passwords), so keep the `/data` volume. Without a settings file they last
until the next restart.

Integrations can also be set with flags or environment variables. Those show
"Set on the command line" and can only be turned off by removing the flags.

| integration | reads | setup |
|---|---|---|
| La Marzocco cloud | time and weight from a connected La Marzocco's coffee log | [src/integration/la_marzocco](src/integration/la_marzocco/README.md) |
| Half Decent Scale | time from first drops and weight, from a Half Decent Scale over WiFi | [src/integration/half_decent](src/integration/half_decent/README.md) |

## Options

Extra flags go after the image name (Docker) or after `--` (cargo):

| flag | default | what |
|---|---|---|
| `--brew-timeout-s <s>` | 180 | give up on a shot with no numbers after this long (up to 3600; 0 waits forever) |
| `--config <file>` | off (Docker: `/data/gbs-anywhere.json`) | settings file for the integrations added in the app; also `CONFIG_FILE` |
| `--icons <dir>` | off | serve `<integration id>.svg/.png` from here instead of the built-in glyphs |
| `--log <file>` | off | append every grinder request to a file |
| `-p, --ports <list>` | 80 | ports to serve on |
| `--grinder <host>` | off | pair with this E65/E80 GbS over its "Generic WiFi" option; also `GRINDER_HOST` |
| `--grinder-pairing-file <file>` | `grinder-pairing.json` next to `--config`; off without `--config` | keeps that pairing across restarts; also `GRINDER_PAIRING_FILE` |
| `--grinder-pairing-id <id>` | off | the pairing ID the grinder shows, used while the saved one does not work (the app can take it too); also `GRINDER_PAIRING_ID` |
| `--lm-*` | off | La Marzocco cloud, see [its README](src/integration/la_marzocco/README.md) |
| `--hds-*` | off | Half Decent Scale, see [its README](src/integration/half_decent/README.md) |

Without Docker you can also type the shot into the console: `30 36` means
30 s, 36 g. `h` lists the other commands.

## Control API

JSON on the same port as the app, for scripts or another front end:

| method | path | what |
|---|---|---|
| GET | `/api/state` | version, phase, machine state, last shot, grinder connection (with `grinder.wifi` for an E65/E80 GbS), integrations |
| GET | `/api/events?after=N` | events with `seq > N` |
| GET | `/api/events/stream` | the same, live, as server-sent events |
| POST | `/api/shot/result` | `{"time_s":30,"weight_g":36}` reports the running shot (time up to 600 s; without a weight, the recipe weight is sent) |
| POST | `/api/shot/abort` | aborts the running shot (the grinder skips it) |
| GET | `/api/integrations/kinds` | the integrations that can be added, with their setup forms |
| POST | `/api/integrations` | `{"kind":"la_marzocco","settings":{"username":"…","password":"…"}}` adds an integration |
| DELETE | `/api/integrations/<id>` | removes an integration added in the app |
| POST | `/api/grinder/pairing` | `{"pairing_id":1234}` gives the pairing ID an E65/E80 GbS shows, only while the app asks for it |

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
