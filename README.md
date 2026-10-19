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

**New here? Start with the [wiki](https://github.com/Skamba/gbs-anywhere/wiki):**
setup for each grinder and integration, daily use and troubleshooting, step
by step. This README is the reference.

> Not affiliated with Mahlkönig or Hemro.

## Run it

On any computer on the same network as the grinder (PC, NAS, Raspberry Pi),
with Docker:

```sh
docker run -d --name gbs-anywhere --restart unless-stopped \
  -p 80:80 -v gbs-anywhere:/data ghcr.io/skamba/gbs-anywhere
```

For an E65/E80 GbS, enter the grinder's IP address in the app's settings
later, or add `-e GRINDER_HOST=<the grinder's IP>` before the image name (see
[Set up an E65/E80 GbS](#set-up-an-e65e80-gbs)).

The `/data` volume keeps what you set up in the app (integrations and
[settings](#settings-in-the-app)), the shot history and the E65/E80 pairing
across restarts, so keep it. From source, with Rust installed,
`cargo run --release -- --config gbs-anywhere.json` does the same.

Give the computer, and an E65/E80 grinder, a fixed IP or a DHCP reservation
in your router: the grinder and your phone need the computer's address (e.g.
`192.168.1.20`). Port 80 must be free and allowed in the firewall.

gbs-anywhere is for your home network: the app has no login, so don't make
it reachable from the internet (no port forwarding to it). `-p 80:80` opens
the port on every network the computer is on; on a NAS or server with more
than one, `-p 192.168.1.20:80:80` keeps it to the home network. Open the app
by the computer's IP address or a home-network name: a name without a dot,
or one ending in `.local`, `.localhost`, `.lan`, `.home`, `.home.arpa`,
`.internal`, `.localdomain` or `.fritz.box`. Any other name, such as a Tailscale
name like `gbs.tail1234.ts.net`, needs `--allow-host`, with the name given in
full. Behind a reverse proxy, pass the `Host` header through and allow the
proxy's name the same way.

`latest` is the newest release; pin one with e.g. `:0.2`, or follow
`:nightly`, built each night from `main`. The app shows its version under
**Status** (and at the bottom of the other tabs); what changed is in
[CHANGELOG.md](CHANGELOG.md).

**Update check.** One minute after it starts, and then once a day,
gbs-anywhere asks GitHub for its latest release, and the gbs-anywhere card
under **Status** says when a newer one is out. GitHub sees the request like
any other, including your IP address; it carries nothing but the
gbs-anywhere version. Turn it off under **Status → Settings**, or with
`--update-check off` (`UPDATE_CHECK=off` in Docker). It goes through a proxy
if `HTTPS_PROXY` is set; the link to an E65/E80 grinder never does.

**To update** with Docker, get the new image and re-create the container
with the same command and volume: `docker pull ghcr.io/skamba/gbs-anywhere`,
`docker rm -f gbs-anywhere`, then the `docker run` above. The `/data` volume
keeps everything you set up.

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
5. Within a few seconds the status at the top right of the app turns
   **Ready** (tap it: **Grinder connected**). The grinder polls every 2
   seconds from then on.

**Make a recipe use Grind-by-Sync**

- New recipe: **Settings → Setup Assistance → Assisted Dial-in**, follow the
  steps, save the recipe in a slot, answer **yes** to "Fine-tune with GbS?"
  and choose **GbS**.
- Existing recipe: open the recipe's menu and turn on **Enable GbS**, then set
  **Brew Time** (target time), **Brew Weight** (target weight in the cup) and
  **GbS start DD** (the grind setting to start from).

A blue chain icon on the recipe means GbS is active.

**Pull a shot**

1. Open `http://<computer's IP>/` on your phone, e.g.
   `http://192.168.1.20/`. While the grinder is polling, the status at the
   top right says **Ready** and **System status** on the Start tab lists
   **Grinder connected**.
2. Grind with a GbS recipe.
3. When the grinder says "Press grinder rotary knob to start brewing.", press
   the knob and start the shot on your machine right away.
4. Time the shot from pump start (always use the same timer), weigh the cup,
   and enter both in the app. Tap **Send**.
5. The grinder shows its new grind setting.

Shots of 10 s or less and over 80 s are ignored by the grinder, as is an
aborted shot.

## Set up an E65/E80 GbS

These grinders don't poll a machine. With their "Generic WiFi" option the
machine pairs with the grinder and answers its request after every
Grind-by-Sync grind. gbs-anywhere does this once it knows the grinder's
address, which is in the grinder's own WiFi settings, e.g. `192.168.1.30`:
enter it in the app's settings, or start gbs-anywhere with it
(`GRINDER_HOST` in Docker, `--grinder` from source). gbs-anywhere talks to the
grinder on port 30001 over HTTPS.

**Pair the grinder**

1. Put the grinder on the same network as the computer running gbs-anywhere.
2. On the grinder, open **Settings → Connectivity → Machine To Machine** and
   enable **Generic WiFi**. Leave the other machines off. Set **Grouphead
   Selection** to **Grouphead 1** (or **All**): Grind-by-Sync only learns
   from shots of the selected group, and gbs-anywhere is a one-group machine.
3. Enter the grinder's address in the app's settings (or start gbs-anywhere
   with it). The app now says to accept gbs-anywhere on the grinder and shows
   a pairing code.
4. On the grinder, in the same **Machine To Machine** menu, open **Generic
   WiFi → Configuration** and tap **Start Pairing**. (If you enabled Generic
   WiFi after starting gbs-anywhere, give it up to a minute to notice.)
   Within a few seconds **gbs-anywhere** shows up under **Available
   devices**, with the same code: tap **Accept Pairing**. If the grinder's
   pairing times out first, tap **Start Pairing** again (the code may
   change: the app always shows the current one).
5. The Generic WiFi entry is now called **gbs-anywhere**, and the app shows
   the grinder as **Ready** under Status. A restart needs no new pairing, and
   neither does a new address for the grinder.

<details>
<summary><b>If pairing doesn't work</b></summary>

The grinder may restart right after you accept (the E65W with firmware
V01.36 does): the screen freezes and the light pulses red, then white. Leave
gbs-anywhere running: it sends the grinder nothing for 3 minutes (the app
shows that the grinder does not answer, and why), because a request while it
starts up can make it lose the new pairing. The same pause follows if the
grinder goes to standby while the pairing code shows. Then open **Generic
WiFi → Configuration** on the grinder:

- **A Pairing ID next to gbs-anywhere:** the pairing is made, but
  gbs-anywhere missed its ID. Type it into the app and tap **Use this pairing
  ID**; gbs-anywhere checks it with the grinder first. (The ID is also under
  **Diagnosis**, after the serial number. `GRINDER_PAIRING_ID` sets it from
  the command line, unchecked.)
- **Start Pairing and no gbs-anywhere entry:** the pairing was lost. Switch
  Generic WiFi on again if needed, then pair once more.

To start over, tap **Clear Pairing** on the grinder, which also switches
Generic WiFi off, and switch it on again. After about a minute gbs-anywhere
asks to pair again by itself.

The wiki's [E65 and E80 GbS page](https://github.com/Skamba/gbs-anywhere/wiki/E65-and-E80-GbS#if-pairing-doesnt-work)
has these steps and other fixes.

</details>

**Make a recipe use Grind-by-Sync** as on the E64 WS: turn on **Enable GbS**
and set **Brew Time**, **Brew Weight** and **GbS start DD**.

**Pull a shot**

There is no knob press: the Grind-by-Sync grind itself tells gbs-anywhere a
shot is coming. Pull the shot and enter its time and weight in the app
(`http://<computer's IP>/`), or let an [integration](#integrations) report
it. The grinder waits for it up to the shot window (3 minutes unless changed
in the app's settings or with `--brew-timeout-s`); if you grind again first,
the shot counts for the newer grind.

The grinder adjusts only from a shot within 15 s of the recipe's **Brew
Time** (for 28 s: over 13 s, up to 43 s), and not from an aborted shot.

**On an ESP32-S3 instead of a computer**

[gbs-anywhere-esp32](https://github.com/zpmdd/gbs-anywhere-esp32) is a
separate, experimental firmware that does the same job on an ESP32-S3 board,
with shots reported by La Marzocco cloud. You pair the grinder with
gbs-anywhere once, then move the pairing to the board. It is a community
project and not part of this repository; please report its issues there.

## Integrations

Entering the numbers on your phone is the default and needs no setup.
Optionally, let an *integration* report the shot instead: a vendor cloud, a
connected scale, a home-automation hub. After the knob press (on an E65/E80
GbS: after the grind) it waits for the shot, then sends its time and weight
to the grinder by itself. If it has not reported the shot a minute after it
should have ended (the recipe's brew time, else 30 s; sooner with a short
shot window), or it is not working, the app says so and shows the fields to
enter the shot on your phone; whichever comes first wins, also when several
integrations run at once.

Under **Status → Machine integrations**, tap the green **+**, pick one and
fill in its form. Each integration gets a card showing whether it is ready
and what it last sent; its ⋮ menu has **Edit**, and **Delete** for those
added in the app. Integrations added in the app are saved, with their
passwords, in `/data` (from source: the `--config` file); without one they
last until the next restart.

An integration added in the app can be changed there too. Its passwords are
never shown again; leave a password empty to keep the saved one.

Integrations can also be set with flags or environment variables. Those show
"From the start settings" and can only be turned off by removing the flags.
To change one in the app, choose **Edit**: with a settings file, the app
offers to move it there, settings and password included, so there is nothing
to type again. From the next start on its flags are ignored for as long as
it is in the app (the log says so at each start, with a warning when they
differ from the app's) and can be removed whenever it suits you. Deleting it
in the app lets the flags start it again at the next start, if they are
still set.

| integration | reads | setup |
|---|---|---|
| La Marzocco cloud | time and weight from a connected La Marzocco's coffee log | [src/integration/la_marzocco](src/integration/la_marzocco/README.md) |
| Half Decent Scale | time from first drops and weight, from a Half Decent Scale over WiFi | [src/integration/half_decent](src/integration/half_decent/README.md) |

## Settings in the app

Three settings can be changed under **Status → Settings**, so the command
line can stay as it is:

- **Time to pull the shot**: how long gbs-anywhere waits for a shot's time
  and weight before it gives up on that shot (`--brew-timeout-s`, 3 minutes
  unless set). In the app it goes from 0:30 to 60:00 minutes, or **No
  limit**. − and + change it in 15 s steps (whole minutes above 10:00). To
  type it, a whole number is minutes up to 60 and seconds above (`3` is
  3:00, `150` is 2:30), and one with decimals is minutes (`2.5` is 2:30),
  which works on a phone's number pad; `m:ss` (`2:30`) and a number with a
  unit (`150 s`) work too. `0` is no limit.
- **Grinder IP address**: the E65/E80 GbS to pair with (`--grinder`,
  `GRINDER_HOST`), optionally with `:port`. A new address takes effect at
  once and keeps the pairing. Saving the field empty removes the address and
  stops pairing, also when `GRINDER_HOST` is set and after a restart
  (without `GRINDER_HOST`, it only forgets the app's address, so one set
  later counts).
- **Update check**: whether gbs-anywhere looks for a newer release once a
  day (see [Update check](#run-it); `--update-check`, `UPDATE_CHECK`, on
  unless set).

Under each one the app says where its value comes from: the default, the
start settings (a flag or environment variable), or set in the app. A
setting changed in the app is saved in the settings file and wins over its
flag from then on; without a settings file it lasts until the next restart.
**Use start setting** forgets the app's value, so the flag or environment
variable counts again, or the default when neither is set. For the grinder
address, that is the way back to `GRINDER_HOST`; an empty field instead
means no grinder, whatever `GRINDER_HOST` says.

In the API (`PUT /api/settings`), `null` goes back to the start setting,
like **Use start setting**. For `grinder_host` that is not the same as
removing the address, which is `""`; without `GRINDER_HOST`, the two are
the same.

**Appearance** (System, Light, Dark) and **Language** (Auto, English,
Deutsch) sit in the same card but are kept on each device: by default the app
follows the device's light or dark mode and its language.

The **History** tab shows the last 200 shots the grinder asked for, with
their time, weight and target, and marks the ones the grinder ignored as
too short or too long (kept in `history.json` next to the settings file).
The **Log** card at the bottom of **Status** (also opened with **Log**
next to the last error in System status) shows the last 1,000 lines of
gbs-anywhere's log (INFO and up, as `docker logs` prints them by default),
ready to copy into a bug report.

## Options

Extra flags go after the image name (Docker) or after `--` (cargo):

| flag | default | what |
|---|---|---|
| `--brew-timeout-s <s>` | 180 | give up on a shot with no numbers after this long (up to 3600; 0 waits forever); **Time to pull the shot** set in the app wins |
| `--config <file>` | off (Docker: `/data/gbs-anywhere.json`) | settings file for what is set up in the app (integrations, settings, integrations moved from the command line); the shot history (`history.json`) is kept next to it; also `CONFIG_FILE` |
| `--icons <dir>` | off | serve `<integration id>.svg/.png` from here instead of the built-in glyphs; also `ICONS_DIR` |
| `-l, --log <file>` | off | append every E64 WS request and the reply to a file |
| `-p, --ports <list>` | 80 | ports to serve on |
| `--allow-host <names>` | off | also open the app by these host names, each in full (e.g. `gbs.tail1234.ts.net`), comma-separated; `*` alone allows any name, even that of a web page that re-points its own name to this address; the IP address and home-network names always work; also `ALLOW_HOST` |
| `--accept-browser-headers` | off | let requests with browser headers reach the E64 WS's routes; only if the log says your E64 WS was refused (then please report it) |
| `--grinder <host>` | off | pair with this E65/E80 GbS over its "Generic WiFi" option; also `GRINDER_HOST`; an address set in the app wins |
| `--grinder-pairing-file <file>` | `grinder-pairing.json` next to `--config`; off without `--config` | keeps that pairing across restarts; also `GRINDER_PAIRING_FILE` |
| `--grinder-pairing-id <id>` | off | the pairing ID the grinder shows, used when none is saved or the grinder refuses the saved one (the app can take it too); also `GRINDER_PAIRING_ID` |
| `--update-check <on\|off>` | on | look for a newer release once a day (see [Update check](#run-it)); also `UPDATE_CHECK`; a choice made in the app wins |
| `--lm-*` | off | La Marzocco cloud, see [its README](src/integration/la_marzocco/README.md) |
| `--hds-*` | off | Half Decent Scale, see [its README](src/integration/half_decent/README.md) |

`gbs-anywhere --help` lists the rest, such as `--bind` and `--no-app`.

Without Docker you can also type the shot into the console: `30 36` means
30 s, 36 g. `h` lists the other commands.

## Control API

Scripts and other front ends can use the app's JSON API on the same port,
e.g. `POST /api/shot/result` with `{"time_s":30,"weight_g":36}`, or follow
`GET /api/events/stream`. All routes are listed in
[`src/server/control_api.rs`](src/server/control_api.rs).

The API has no login: anyone on your network who can open the app can add,
change or remove integrations, change the settings, including the grinder
address, and read the log, as they can report shots. Passwords are never
sent back.
A web page opened on a phone in your network cannot use the API: it answers
only to home-network host names and refuses changes requested from other
sites, and the grinder's routes refuse changes from browsers. Such a page
can at most read the machine state the grinder polls, which holds nothing
private.

## Build and contribute

`cargo build --release` builds `target/release/gbs-anywhere`, and
`docker build -t gbs-anywhere .` the image. [CONTRIBUTING.md](CONTRIBUTING.md)
covers the checks, the conventions and adding an integration.

## License

Copyright 2026 Skamba and the gbs-anywhere contributors.

[GNU AGPL v3 or later](LICENSE): anyone, cafés included, may use, change and
share it. If you share a changed version, or let other people use one over a
network, you must offer them its source code under the same license.

The image also carries the licenses of the Rust crates built into it, in
`/usr/share/doc/gbs-anywhere/THIRD-PARTY-NOTICES`
(`docker cp gbs-anywhere:/usr/share/doc/gbs-anywhere/THIRD-PARTY-NOTICES .`
copies it out). From source, `cargo about generate about.hbs` prints the same
list, with [cargo-about](https://github.com/EmbarkStudios/cargo-about)
installed.
