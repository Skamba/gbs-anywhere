# gbs-anywhere

Grind-by-Sync for the Mahlkönig **E64/E65/E80** GbS grinders
(**E64 WS, E65W, E65T, E80W, E80T**) with any espresso machine

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
| E65W, E65T, E80W, E80T GbS | gbs-anywhere pairs with the grinder over its Local WiFi | [Set up an E65/E80 GbS](#set-up-an-e65e80-gbs) |

Tested with the E64 WS and the E65W GbS; the other E65/E80 GbS models offer
the same Local WiFi pairing.

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

Then open the app and tap **Set up grinder ›** on the Start tab: it asks
which grinder you have and takes you through its steps. For an E65/E80 GbS
you can instead add `-e GRINDER_HOST=<the grinder's IP>` before the image
name (see [Set up an E65/E80 GbS](#set-up-an-e65e80-gbs)).

The `/data` volume keeps what you set up in the app (integrations and
[settings](#settings-in-the-app)), the shot history and the E65/E80 pairing
across restarts, so keep it. From source, with Rust installed,
`cargo run --release -- --config gbs-anywhere.json` does the same, with two
differences: on Linux, port 80 needs root or `CAP_NET_BIND_SERVICE` (or pick
another port with `-p`, for an E65/E80), and the console prompt and a
second copy of the app and API on `127.0.0.1:8787`, for this computer
only, are on; the image turns both off (`--no-stdin --control off`).

**Without Docker**, on a Raspberry Pi with 64-bit Raspberry Pi OS (Pi 3,
4, 5, Zero 2 W), Debian 12 or newer, or Ubuntu 24.04 or newer, install the
Debian package from the
[latest release](https://github.com/Skamba/gbs-anywhere/releases/latest):
`arm64` for a Raspberry Pi, `amd64` for a PC. In the folder you downloaded
it to:

```sh
sudo apt install ./gbs-anywhere_<version>-1_arm64.deb
```

This runs gbs-anywhere as the `gbs-anywhere` service on port 80 and starts
it at boot, with the same flags as the image. What you set up in the app,
the shot history and the E65/E80 pairing are kept in
`/var/lib/gbs-anywhere`. Environment variables such as `GRINDER_HOST`, and
extra flags in `GBS_ANYWHERE_ARGS` (say `GBS_ANYWHERE_ARGS=-p 8080`), go in
`/etc/default/gbs-anywhere`; run `sudo systemctl restart gbs-anywhere`
after changing it. `journalctl -u gbs-anywhere` shows the log. To update,
install the newer package the same way. `sudo apt remove gbs-anywhere`
keeps your data, and `sudo apt purge gbs-anywhere` deletes it as well. Each
release also has the plain binary
(`gbs-anywhere-<version>-linux-<arch>.tar.gz`), for other Linux systems
with glibc 2.36 or newer.

Give the computer a fixed IP or a DHCP reservation in your router, as your
phone needs its address (e.g. `192.168.1.20`). Then, depending on the
grinder:

- **E64 WS:** the grinder needs the computer's address too, and polls it
  on port 80, so port 80 must be free and allowed in the firewall.
- **E65/E80 GbS:** gbs-anywhere polls the grinder (on its port 30001), so
  give the grinder a fixed IP or reservation as well. The app can then run
  on any port (`-p 8080:80` in Docker, `-p 8080` from source), which only
  your phone needs to reach.

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

`latest` is the newest release; pin one with e.g. `:0.3`, or follow
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
with the same command and volume:
`docker pull ghcr.io/skamba/gbs-anywhere` (or the tag you run),
`docker rm -f gbs-anywhere`, then the `docker run` above. The `/data` volume
keeps everything you set up, and an app left open reloads itself once the
new version answers and nothing is being typed in it.

## Set up the E64 WS

The grinder treats gbs-anywhere as a Xenia, so the menus below say "Xenia".

**Connect the grinder**

The app takes you through these steps too: tap **Set up grinder ›** on the
Start tab and choose **E64 WS**. It shows the address to enter, also when
the app was opened by name, and it says when the grinder is connected. In
Docker it sees only the container's own address (unless the container uses
the host's network): when the address is in Docker's range (172.17.x.x to
172.31.x.x), it says so and asks for the computer's address, which your
router's list of devices shows.

1. Put the grinder on the same network as the computer running gbs-anywhere.
   The E64 WS only supports 2.4 GHz WiFi.
2. On the grinder, open the main menu (lower-left button on the home screen).
3. Go to **Connectivity → Machine To Machine → Enable Xenia**.
4. Open **Configuration → Edit Host**. The scan will not find gbs-anywhere,
   so enter the hostname by hand: the computer's IP address, e.g.
   `192.168.1.20`. An IP is more reliable than a `.local` name.
5. Within a few seconds the status at the top right of the app turns
   **Ready** (tap it: **Grinder connected**). The grinder polls every 2
   seconds from then on.

**Make a recipe use Grind-by-Sync**

- New recipe: main menu → **Setup Assistance → Assisted Dial-in**, follow the
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

These grinders don't poll a machine. Over the grinder's Local WiFi (under
**Machine To Machine → GBS COFFEE MACHINES**) the machine pairs with the
grinder and answers its request after every Grind-by-Sync grind.
gbs-anywhere does this once it knows the grinder's address, which is in the
grinder's own WiFi settings, e.g. `192.168.1.30`: enter it in the app, or
start gbs-anywhere with it (`GRINDER_HOST` in Docker, `--grinder` from
source). gbs-anywhere talks to the grinder on port 30001 over HTTPS.

**Pair the grinder**

1. Put the grinder on the same network as the computer running gbs-anywhere.
   On the grinder, leave the other machines under **GBS COFFEE MACHINES**
   off, and set **Grouphead Selection** to **Grouphead 1** (or **All**):
   Grind-by-Sync only learns from shots of the selected group, and
   gbs-anywhere is a one-group machine.
2. On the Start tab, tap **Set up grinder ›** under System status and
   choose **E65 / E80**: the setup goes on with the grinder's address.
   With the address already set
   (with **Change IP address** in the ⋮ menu of the grinder card under
   **Status**, or when starting gbs-anywhere), System status says the
   grinder is not set up, or that it is pairing once the grinder shows a
   Pairing Code: tap **Set up grinder ›** there, or **Set up** on the
   grinder card under Status.
3. **Set up grinder** takes you through five steps with the grinder's own
   menu names: enable Local WiFi, start pairing under **Configuration**,
   compare the **Pairing Code** in the app with the grinder's and accept
   **gbs-anywhere** on the grinder, then wait while the grinder may restart,
   and confirm the pairing. Keep your phone at the grinder. The steps
   follow what gbs-anywhere sees and what you answered, so a reload picks
   up where you were.

A restart needs no new pairing, and neither does a new address for the
grinder. The grinder card under Status then shows the **Pairing ID**; to
pair again, **Clear pairing** in its ⋮ menu opens the setup, which starts
by clearing the pairing on the grinder.

<details>
<summary><b>If pairing doesn't work</b></summary>

gbs-anywhere checks the pairing 30 s after you tap **Accepted** in the
app, and sends the grinder nothing until then. Without the tap, it checks
every 40 s, and the setup keeps waiting for the tap. If the grinder hasn't
accepted gbs-anywhere by the check, gbs-anywhere waits for the accept and a
new tap. If the grinder lists gbs-anywhere more than once, accept the
entry with the Pairing Code the app shows; it may be further down. If the
grinder shows **Timeout** next to **Start Pairing**, its
pairing search ended: tap **Start Pairing** again. The grinder may freeze
and restart: its LED pulses red, then
white. When a pairing check goes unanswered, gbs-anywhere
sends the grinder nothing for 3 minutes, then checks again; the setup waits
with you (or, while you restart the grinder after a pairing that wasn't
saved, goes on with the next try). Then:

- **Configuration shows a Pairing ID** (and gbs-anywhere as the name): the
  pairing is made. The setup asks for that number and checks it with the
  grinder. (It is also under **Diagnosis**, after the serial number.
  `GRINDER_PAIRING_ID` sets it from the command line, unchecked.)
- **No Pairing ID under Configuration:** the pairing wasn't kept. It can't
  be brought back; the setup says so and asks you to restart the grinder
  before the next try, then starts again at step 2 with a new Pairing Code.
  The restart also clears the grinder's list of pairing requests, so it
  shows only the new one. **Enable** may be off again after the restart.
  It can take several tries.

A pairing seems to stick more easily on an idle grinder: don't grind right
before or while you pair, and leave the grinder alone after you accept
until it has fully started again. Once a pairing sticks, avoid **Clear
Pairing**, as it means pairing anew.

The wiki's [E65 and E80 GbS page](https://github.com/Skamba/gbs-anywhere/wiki/E65-and-E80-GbS#if-pairing-doesnt-work)
has the long version and other fixes.

</details>

**Make a recipe use Grind-by-Sync** as on the E64 WS: turn on **Enable GbS**
and set **Brew Time**, **Brew Weight** and **GbS start DD**.

**Pull a shot**

There is no knob press: the Grind-by-Sync grind itself tells gbs-anywhere a
shot is coming. Pull the shot and enter its time and weight in the app
(`http://<computer's IP>/`), or let an [integration](#integrations) report
it. The grinder waits for it up to the shot window (3 minutes unless changed
in the app's settings or with `--shot-window-s`); if you grind again first,
the shot counts for the newer grind.

The grinder adjusts only from a shot over 10 s and within 15 s of the
recipe's **Brew Time** (for 28 s: over 13 s, up to 43 s; for 20 s: over
10 s, up to 35 s), and not from an aborted shot.

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

Under **Status → Machine integrations**, tap **+**, pick one and
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

Three settings can be changed in the app, so the command line can stay as
it is: the grinder's address on the grinder card under **Status** (its ⋮
menu), the other two under **Status → Settings**.

- **Time to pull the shot**: how long gbs-anywhere waits for a shot's time
  and weight before it gives up on that shot (`--shot-window-s`, 3 minutes
  unless set). In the app it goes from 0:30 to 60:00 minutes, or **No
  limit**. − and + change it in 15 s steps (whole minutes above 10:00). To
  type it, a whole number is minutes up to 60 and seconds above (`3` is
  3:00, `150` is 2:30), and one with decimals is minutes (`2.5` is 2:30),
  which works on a phone's number pad; `m:ss` (`2:30`) and a number with a
  unit (`150 s`) work too. `0` is no limit.
- **Grinder IP address** (**Enter IP address** or **Change IP address** in
  the grinder card's ⋮ menu): the E65/E80 GbS to pair with (`--grinder`,
  `GRINDER_HOST`), optionally with `:port`. A new address takes effect at
  once and keeps the pairing. Saving the field empty removes the address and
  stops pairing, also when `GRINDER_HOST` is set and after a restart
  (without `GRINDER_HOST`, it only forgets the app's address, so one set
  later counts).
- **Update check**: whether gbs-anywhere looks for a newer release once a
  day (see [Update check](#run-it); `--update-check`, `UPDATE_CHECK`, on
  unless set).

Under each one (for the address: in its form) the app says where its value
comes from: the default, the start settings (a flag or environment
variable), or set in the app. A
setting changed in the app is saved in the settings file and wins over its
flag from then on; without a settings file it lasts until the next restart.
**Use start setting** forgets the app's value, so the flag or environment
variable counts again, or the default when neither is set. For the grinder
address, that is the way back to `GRINDER_HOST`, offered only when it is
set; an empty field instead means no grinder, whatever `GRINDER_HOST` says.

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

Extra flags go after the image name (Docker), in `GBS_ANYWHERE_ARGS` (Debian
package) or after `--` (cargo). A flag given again wins over the one before
(lists such as `--allow-host` add up instead), so `--control 0.0.0.0:8787`
there turns on the control address the image and the package turn off:

| flag | default | what |
|---|---|---|
| `--shot-window-s <s>` | 180 | give up on a shot with no numbers after this long (30 to 3600, as in the app; 0 waits forever); **Time to pull the shot** set in the app wins; `--brew-timeout-s`, its name until 0.2.1, still works |
| `--config <file>` | off (Docker: `/data/gbs-anywhere.json`, package: `/var/lib/gbs-anywhere/gbs-anywhere.json`) | settings file for what is set up in the app (integrations, settings, integrations moved from the command line); the shot history (`history.json`) is kept next to it; also `CONFIG_FILE` |
| `--icons <dir>` | off | serve `<integration id>.svg/.png` from here instead of the built-in glyphs; also `ICONS_DIR` |
| `-l, --log <file>` | off | append every E64 WS request and the reply to a file, readable by its owner only; past 4 MiB it moves to `<file>.1` (replacing the one there) and starts over |
| `-p, --ports <list>` | 80 | ports to serve on, comma-separated; the E64 WS polls port 80 |
| `-b, --bind <addr>` | `0.0.0.0` | address to serve those ports on |
| `--control <addr>` | `127.0.0.1:8787` (Docker and package: `off`) | a second address for the app and its API, besides the ports above; `off` turns it off |
| `--no-stdin` | off (Docker and package: on) | no console prompt, for running as a service |
| `--allow-host <names>` | off | also open the app by these host names, each in full (e.g. `gbs.tail1234.ts.net`), comma-separated; `*` alone allows any name, even that of a web page that re-points its own name to this address; the IP address and home-network names always work; also `ALLOW_HOST` |
| `--finishing-hold-s <s>` | 6 | how long the machine shows a shot as finishing before it is ready again, 3 to 60: the E64 WS polls every 2 s and takes the shot only from a poll that sees it finish, and the next shot waits for it |
| `--serial <sn>` | built in | the machine serial number the E64 WS is shown |
| `--accept-browser-headers` | off | let requests with browser headers reach the E64 WS's routes; only if the log says your E64 WS was refused (then please report it) |
| `--grinder <host>` | off | pair with this E65/E80 GbS over its Local WiFi; also `GRINDER_HOST`; an address set in the app wins |
| `--grinder-pairing-file <file>` | `grinder-pairing.json` next to `--config`; off without `--config` | keeps that pairing across restarts; also `GRINDER_PAIRING_FILE` |
| `--grinder-pairing-id <id>` | off | the Pairing ID the grinder shows, used when none is saved or the grinder refuses the saved one (the app's setup can take it too); also `GRINDER_PAIRING_ID` |
| `--update-check <on\|off>` | on | look for a newer release once a day (see [Update check](#run-it)); also `UPDATE_CHECK`; a choice made in the app wins |
| `--lm-*` | off | La Marzocco cloud, see [its README](src/integration/la_marzocco/README.md) |
| `--hds-*` | off | Half Decent Scale, see [its README](src/integration/half_decent/README.md) |

`gbs-anywhere --help` lists the rest, such as `--no-app`.

Without Docker you can also type the shot into the console: `30 36` means
30 s, 36 g. `h` lists the other commands.

## Control API

Scripts and other front ends can use the app's JSON API on the same port,
e.g. `POST /api/shot/result` with `{"time_s":30,"weight_g":36}`, or follow
`GET /api/events/stream` (reconnected with `Last-Event-ID`, it first sends
the events missed meanwhile; event numbers start over when gbs-anywhere
restarts, which `server.boot_id` in `/api/state` tells). All routes are listed in
[`src/server/control_api.rs`](src/server/control_api.rs).
`GET /api/state` comes in groups (`shot`, `grinder`, `machine`,
`integrations`, `settings`, `update`, `server`), and an error replies
`{"error":"<sentence>","code":"<code>"}`, with a fixed `code` to match on.
The same goes for the rest: match on the codes (`code`, `cause`, `kind`,
`state`, `phase`, and `problem_cause` for an E65/E80 link's last problem);
`reason`, `detail`, `problem` and `error` are English words for people.
Lengths and ages are measured on the server, so they hold when its clock
is not set: each event and log line says how long ago it happened
(`age_ms`), a running shot how long it has run (`shot.phase_age_ms`), and
`shot_reported`, `shot_aborted` and the history how long a brew ran
(`running_ms`). `shot_reported` and the history also name the
integration that reported a shot (`integration_id`).
Coming from 0.2.1, whose names were different, the
[changelog](CHANGELOG.md#changed-breaking) lists what was renamed.

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

`cargo build --release` builds `target/release/gbs-anywhere`,
`docker build -t gbs-anywhere .` the image, and
`docker run --rm -v "$PWD:/src" -w /src rust:1-bookworm sh packaging/build-release.sh`
the release tarball and Debian package in `dist/`. [CONTRIBUTING.md](CONTRIBUTING.md)
covers the checks, the conventions and adding an integration.

## License

Copyright 2026 Skamba and the gbs-anywhere contributors.

[GNU AGPL v3 or later](LICENSE): anyone, cafés included, may use, change and
share it. If you share a changed version, or let other people use one over a
network, you must offer them its source code under the same license.

The image and the Debian package also carry the licenses of the Rust
crates built into them, in `/usr/share/doc/gbs-anywhere/THIRD-PARTY-NOTICES`
(`docker cp gbs-anywhere:/usr/share/doc/gbs-anywhere/THIRD-PARTY-NOTICES .`
copies it out of the container); the release tarballs have it next to the
binary. From source, `cargo about generate about.hbs` prints the same
list, with [cargo-about](https://github.com/EmbarkStudios/cargo-about)
installed.
