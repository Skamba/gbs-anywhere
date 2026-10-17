# Changelog

Notable changes for people running gbs-anywhere, newest first. Versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0,
a minor release may change behaviour.

## [Unreleased]

### Added

- E65/E80 GbS grinders (E65W, E65T, E80W, E80T): `--grinder <address>`
  (`GRINDER_HOST` in Docker) pairs gbs-anywhere with the grinder's "Generic
  WiFi" option. After each Grind-by-Sync grind, the shot entered in the app
  or reported by an integration goes to the grinder, which adjusts as usual.
  The pairing is kept next to the settings file (`--grinder-pairing-file`).
  If the grinder restarts as the pairing is accepted (the E65W with firmware
  V01.36 does), gbs-anywhere sends it nothing for 3 minutes while it starts
  up, so that it keeps the pairing; the app then asks for the ID the grinder
  shows (also `--grinder-pairing-id`). The app shows the pairing code while
  pairing and the grinder as ready once paired. The grinder waits for the
  shot for up to `--brew-timeout-s` (3 minutes unless set); grinding again
  before pulling cancels the shot it was waiting for. A shot that cannot
  reach the grinder right away (say, the WiFi drops) is retried for two
  minutes. While the grinder is off or in standby, the log says so once
  rather than every minute. Tested with the E65W GbS.
- `grinder.wifi.pairing_id` in `GET /api/state`: the E65/E80 pairing in use;
  `grinder.wifi.unknown_pairing_id`: one typed in that the grinder did not
  take, while the app asks for the ID.
- `grinder.wifi.unreachable` in `GET /api/state`: true while an E65/E80 GbS
  does not answer at all (off, in standby or gone from the network), as
  opposed to answering with an error.
- `boot_id` in `GET /api/state`: new on every start, so an app can tell that
  event numbers started over.
- `age_ms` on each event in `GET /api/events` and the event stream: how long
  ago it happened, so an app that catches up after a gap shows the right
  times.
- A redesigned app with two tabs: Start (the three steps of a shot, with
  the step that is running or failed opened) and Status (grinder,
  integrations). When no shot can start (no grinder connected, or the
  grinder blocked), System status opens by itself and says why in plain
  words, and step 1 shows "Blocked" instead of "Grind". While an
  integration reports the shot, the fields to type it in are hidden; they
  come back, with a note, when every integration has failed or none has
  reported the shot a minute after it should have ended. The list of this
  session's shots is gone for now; a shot history will replace it.

### Changed

- Unreleased builds are now published once a night as the `nightly` image
  tag, and only on nights when something changed. The `main` and `sha-*`
  tags are no longer updated; switch from `:main` to `:nightly`. `latest`
  and the version tags still come only from releases.

### Fixed

- Requests on port 80 outside the grinder's `/api/v2/` paths, such as other
  devices on the network probing the port, no longer count as grinder
  traffic and are logged at debug level only. The `--log` transcript still
  records them.

## [0.2.1] - 2026-10-10

### Changed

- Integration cards say "ready" instead of "connected" when idle. The Half
  Decent Scale is only connected while the grinder waits, so "connected"
  was wrong between shots. A scale not reached at startup no longer shows
  as off: it may have been switched on since.

## [0.2.0] - 2026-10-09

### Added

- Half Decent Scale integration: reports each shot weighed on a Half Decent
  Scale over WiFi (firmware 3.0 or newer), timed from the first drops to the
  flow stopping. It tares the scale at the knob press, and is connected only
  while the grinder waits, so the scale still switches itself off. Add it
  with the green + or `--hds-host`.

## [0.1.1] - 2026-09-30

### Fixed

- A shot entered without a weight now gets the recipe weight, as shots from
  integrations already did, instead of sending 0 g to the grinder. With no
  recipe weight either, the shot is refused and the app asks for the weight.
- Opening the app no longer counts as grinder traffic: the browser's icon
  requests used to show up as grinder requests in the state and the log.
- A shot time over 600 s, or a `--brew-timeout-s`, `--finishing-hold-s` or
  La Marzocco check interval out of range, is now refused with a message
  instead of crashing the request or the app. The check interval is now 1 to
  60 s.
- The La Marzocco card goes back to "watching" when the coffee log answers
  again after a failed check, instead of showing the error for the rest of the
  brew and telling you to enter the shot by hand.

## [0.1.0] - 2026-09-28

First release.

### Added

- Stands in for the espresso machine the E64 WS syncs with, so Grind-by-Sync
  works with any espresso machine: pull the shot, enter time and weight on
  your phone, and the grinder adjusts its grind setting.
- Phone app on port 80 with the shot flow, the grinder's connection state and
  this session's shots.
- Integrations that report shots by themselves, added in the app with the
  green **+** (saved in `/data`) or with command-line flags; several can run
  side by side.
- La Marzocco cloud integration: time and weight from a connected
  La Marzocco's coffee log (contributed by @rvdh).
- JSON control API and a live event stream.
- Docker image for amd64 and arm64 on `ghcr.io/skamba/gbs-anywhere`.
- Version shown in the app, in `GET /api/state` and in the startup log.
