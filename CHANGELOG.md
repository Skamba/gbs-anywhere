# Changelog

Notable changes for people running gbs-anywhere, newest first. Versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0,
a minor release may change behaviour.

## [Unreleased]

### Changed (breaking)

`grinder.wifi` in `GET /api/state` (and in the replies that carry the same
state) now says where an E65/E80 pairing is in one `phase`. Scripts written
for 0.3.x that read it need the changes below; the pairing file is read as
before.

| 0.3.x | now |
|---|---|
| `state: "pairing"`: the Pairing Code is shown | only with `phase: "accepting"`; `pairing` also covers `holding`, `checking`, `quiet` and `asking` |
| `needs_pairing_id: true`, with `state: "failing"` | `phase: "asking"`, with `state: "pairing"` |
| `state: "failing"` while nothing is sent for 3 minutes after an unanswered pairing check | `state: "pairing"`, `phase: "quiet"` (`unreachable` stays `true`) |
| `POST /api/grinder/pairing` while not paired and not asking for an ID: `grinder_already_paired` | `grinder_not_asking`; `grinder_already_paired` only while paired |

- `--finishing-hold-s` takes 3 to 60 seconds (was 0 to 3600): with a hold
  shorter than an E64 WS poll, the grinder missed shots that the app said
  were sent.
- `--shot-window-s` takes 0 or 30 to 3600 seconds, as the app does (1 to 29
  were taken before).

### Added

- **Set up grinder**: the app sets up the grinder step by step, in the
  grinder's own menu names, from **Set up grinder ›** in System status on
  the Start tab or **Set up** on the grinder card. While no grinder is
  known yet, it first asks which one you have. An E64 WS is shown
  where to enable Xenia and which address to enter (the IP the app was
  opened at, when it was opened at one), and the setup ends once the
  grinder polls. An E65/E80 GbS is paired; this replaces the pairing card
  and the Pairing ID form on the Start tab. A reload keeps your answers.
  Once an E65/E80 is paired, the setup asks whether the grinder shows
  Status On, and starts again from step 2 if not. When a pairing wasn't
  saved, it asks you to restart the grinder before the next try, which also
  clears the grinder's list of pairing requests, and it says which entry to
  accept if gbs-anywhere is listed more than once.
- A ⋮ menu on the grinder card under Status: **Change IP address**
  (**Enter IP address** while none is set) and, while an E65/E80 is paired,
  **Clear pairing**, which opens the setup to pair again.
- `POST /api/grinder/accepted`: the app's "I accepted on the grinder".
  gbs-anywhere checks the pairing 30 s later and sends the grinder nothing
  meanwhile; without it, it checks every 40 s (was every 2 s). Taken only
  in phase `accepting`, else refused with `grinder_not_accepting`.
- `grinder.wifi.phase` while `state` is `pairing`: `accepting` (the Pairing
  Code is shown), `holding` (confirmed, waiting 30 s), `checking`, `quiet`
  (nothing sent for 3 minutes after an unanswered check) or `asking` (the
  Pairing ID is needed); `phase_age_ms` says how long it has been in it,
  and `quiet_ms` how long a `quiet` round lasts.
- `grinder_not_asking`: `POST /api/grinder/pairing` while the link is not
  paired but not asking for a Pairing ID either.
- `grinder.wifi.state_age_ms`, how long the link has been in its `state`,
  and `grinder.wifi.not_set_up`: not paired, and the grinder answers, but
  turns the pairing application down with a client error (as one not set
  up for pairing would). System status and the grinder card say the grinder
  is not set up only then; any other error while not paired shows as an
  error, with its detail and the way to the log.

- Opened by name (such as `gbs-anywhere.local`), the E64 WS setup and
  the grinder card under Status show the address to enter on the grinder
  too, as the server sees it. When that looks like an address inside
  Docker, which the grinder can't reach, the app says so instead. In the
  API: `server.ip` and `server.ip_in_docker_range` in `/api/state`.
- The control API says how long things took or happened, as the server
  measured them, so apps don't depend on its clock: `shot.phase_age_ms`
  while a shot runs, `running_ms` on `shot_reported`, `shot_aborted` and
  each history entry (for shots saved by earlier builds, from their two
  times), and `age_ms` on each log line in `/api/log` and its stream.
- `shot_reported` and the shot history name the integration that reported a
  shot (`integration_id`; `null` when typed in the app or sent through the
  API). The history shows it next to the target.
- `grinder.wifi` has `problem_cause`, the last shot's problem as a code (an
  abort cause such as `grinder_shot_dropped`, or `grinder_result_expired`,
  `grinder_result_refused`, `grinder_result_delayed`), and `error`: while
  the link fails, why, without the steps before it.

### Changed

- The grinder's address moved from Status → Settings into the grinder
  card's ⋮ menu, with where it comes from in its form. **Use start
  setting** is offered there only when there is a start address.
- **Add** and **Save** in the integration forms and the **+** for a new
  integration use the accent colour like the other main buttons, and
  warnings (an aborted shot in History, warning notes) are orange, apart
  from the brown accent.
- After a shot, step 3 on the Start tab stays open with the shot, the
  grinder's rating and what to expect next; a closed step shows what it
  came to under its title (the recipe target, the shot, the rating with its
  trend), and the step heads keep their place when opened or closed.
- The appearance and language switches under Settings have equal widths,
  and the pressed label no longer wraps or shifts them.
- While not paired, a grinder that does not answer is tried again at least
  every 10 s (was up to a minute), so a Pairing Code follows soon after it
  answers again.
- The app, the README, the log and `--grinder`'s help call the link Local
  WiFi, after the line under the grinder's switch (Local WiFi connected),
  instead of "Generic WiFi", and the pairing hints use the grinder's words
  (Configuration, Start Pairing, Pairing Code). Without a grinder, the
  refusal points to **Set up grinder**.
- A flag given again now wins over the one before, so `--control <addr>`
  after the Docker image name or in `GBS_ANYWHERE_ARGS` turns on the control
  address that the image and the Debian package turn off. Lists such as
  `--allow-host` still add up.
- The `--log` file is readable by its owner only. Past 4 MiB it moves to
  `<file>.1` and starts over, and each request stays on one shortened line.
- `/api/events/stream` resumes from `Last-Event-ID` (or `?after=N`) like
  `/api/log/stream`, and no longer skips events when it falls behind.
- La Marzocco cloud: a refused login, or a machine it can't pick (none on
  the account, the serial number not on it, or more than one and no serial
  number), is tried again after an hour once it was refused for 2 minutes
  in a row, instead of every 5 minutes forever; changing its settings in the
  app tries again at once. The card says when.
- The app now uses the same words for the grinder's state everywhere. A
  Pairing Code waiting to be accepted shows as "Pairing", not "Grinder not
  set up". A grinder that doesn't answer within 15 s counts as not
  connected, also in the setup's step 1. A paired E65/E80 GbS with a problem
  shows "Problem" on the grinder card.
- The ⋮ menus on the Status tab close when the focus moves on past them, and
  screen readers now announce them as a button that shows a list rather than
  as a menu.
- An `age_ms` among an integration's `detail_vars` is as of the reply, so "3
  min ago" counts on in the app.

### Fixed

- The log in the app shows each line's time again, and **Copy** finds the
  lines of the last hour; both were broken since 0.3.0.
- Unknown or unparsable requests under `/api/v2` no longer change the
  grinder address shown in the app, and are logged once per device and path
  rather than every time, so they can't push the rest out of the app's log.
- An invalid `--control` address, or one already in use, now names the flag
  and the address, and is reported before anything starts.
- A `--log` file that can't be opened now says which file.
- La Marzocco cloud: a coffee made before the knob press is no longer
  reported as the shot when the cloud can't be reached at the knob press.
  Coffees are compared on the cloud's own clock, and a shot is missed (type
  it in the app) rather than reported wrong.
- La Marzocco cloud: when the next knob press came within one check
  interval, the previous shot's coffee could be reported for the new one.
  Each knob press is now its own brew, also for the Half Decent Scale.
- La Marzocco cloud: when La Marzocco asks to confirm its updated terms
  and privacy policy, the card says to confirm them in the La Marzocco app,
  instead of only "403 Forbidden". An error that keeps coming back is logged
  once and then once an hour, rather than at every try (every 3 s during a
  brew, up to every 5 minutes otherwise).
- A knob press made while the La Marzocco cloud integration is logging in or
  retrying is no longer missed.
- The La Marzocco card no longer shows "last coffee 0 s ago" when the
  computer's clock is behind or not set; it leaves the age out.
- An integration that stops on an internal error now shows as failing with
  the error, and the error appears in the app's log.
- A shot time typed for one shot no longer stays for the next one when the
  phone slept or lost the network in between.
- Tapping or focusing a filter on the History tab, an open ⋮ menu, or an
  answer in the setup no longer loses the focus when the page updates.
- Screen readers no longer repeat the hint under the shot time on every
  keystroke, or an error message that didn't change.
- A setting changed under Status → Settings (update check, time to pull the
  shot) no longer flicks back to its old value for a moment.
- The setup's progress bars fill the whole row for the E64 WS's three steps.
- Icons from `--icons` show in the list of integrations to add the first
  time it opens.
- The app shows request errors in your language, also when gbs-anywhere does
  not answer (it said "Failed to fetch" or "Load failed").
- The app words an E65/E80 problem and a blocked grind in your language
  instead of the server's English, and shows a problem once when the failed
  shot has the same cause.
- "Reported by …" names the integration that reported the shot, instead of
  guessing from the time and weight.
- Copying the last hour of the log works when the gbs-anywhere computer's
  clock is not set.
- The shot timer keeps running when the machine values are overridden, and a
  timeout in the history no longer shows the current time limit for an older
  shot.

## [0.3.1] - 2026-10-24

The app is the same as in 0.3.0; this release adds a way to run it without
Docker.

### Added

- Each release has a Debian package for `arm64` (Raspberry Pi) and `amd64`,
  which runs gbs-anywhere as a systemd service without Docker, and the plain
  binary as a tarball. See
  [Run it](https://github.com/Skamba/gbs-anywhere#run-it).

## [0.3.0] - 2026-10-24

### Changed (breaking)

The API's names now follow one pattern, before more apps come to rely on
them: `GET /api/state` comes in groups (`shot`, `grinder`, `machine`,
`integrations`, `settings`, `update`, `server`), an event is named by what
happened to whom (`grinder_shot_requested`, `shot_started`), a state is one
word, and units are in the name (`_ms`, `_s`, `_g`). Scripts and
automations written for 0.2.1 need the changes below; the app, the grinder
and shots typed in on the phone are not affected. `--brew-timeout-s` and
`volume_ml` in `POST /api/shot/result` are still accepted, and a shot
history saved by a nightly build is read as before. Going back to a nightly
build from before this change loses the shot history, as those builds
can't read the names it is now saved with.

`GET /api/state`, and the same state as the reply of `POST /api/shot/*`,
`POST /api/machine` and `PUT`/`DELETE /api/overrides`:

| 0.2.1 | now |
|---|---|
| `phase.phase`: `ready`, `brewing`, `finishing` (with `requested_by_grinder`, `result`, `aborted`) | `shot.phase`: `ready`, `running`, `finishing` (with the same fields in `shot`) |
| `last_shot` | `shot.last` |
| `volume_ml` in a shot's result (`phase.result`, `last_shot`, `integrations[].last_report`) | `weight_g` |
| `last_grind` (`beverage_weight_g`, `filter`, `raw`) | `grinder.last_grind` (`target_weight_g`, `filter`, `raw`) |
| `grind_blocked.reason`: `standby`, `eco`, `brewing`, `testing`, `fault`, `warming_up`, `tank_empty`, `not_connected` | `grinder.grind_blocked.cause`: `machine_in_standby`, `machine_in_eco`, `shot_running`, `machine_testing`, `machine_faulty`, `machine_warming_up`, `machine_tank_empty` (`not_connected` was never sent) |
| `grinder` (`peer`, `polls`, `requests`, `last_poll_age_ms`) | the same, in `grinder` |
| `mako`, `overrides`, `config` | `machine.mako`, `machine.overrides`, `machine.config` |
| `config.brew_timeout`, `config.finishing_hold` (`{"secs":…,"nanos":…}`) | `machine.config.shot_window_s` (whole seconds, 0 for no limit, as in `settings`), `machine.config.finishing_hold_s` (seconds, as `--finishing-hold-s` takes them) |
| `health` of an integration: `error` | `failing` |
| `since_ms` of an integration | `health_age_ms` |
| `last_seq`, `version` | `server.last_event_seq`, `server.version` |

Events (`GET /api/events`, `GET /api/events/stream`):

| 0.2.1 | now |
|---|---|
| `grind_result` (`result.beverage_weight_g`) | `grinder_grind_finished` (`result.target_weight_g`) |
| `start_requested` | `grinder_shot_requested` (for an E65/E80 too, with `target_time_ms`) |
| `brew_started` | `shot_started` |
| `shot_reported` (`result.volume_ml`) | `shot_reported` (`result.weight_g`) |
| `shot_reported`: `in_grinder_range`, `true` or `false` (`false` only over 80 s) | `true`, `false` or `null`: by the window of the grinder's request (for the E64 WS over 10 s and up to 80 s, see `grinder.shot_range`), `null` when that is not known |
| `shot_aborted` | unchanged, with `cause` added |
| `ready` | `shot_finished` |

Elsewhere:

| 0.2.1 | now |
|---|---|
| `POST /api/shot/result` with `volume_ml` | `weight_g` (`volume_ml` still works) |
| `DELETE /api/integrations/<id>` replies `{"removed":"<id>"}` | `{"id":"<id>"}`, as adding one does |
| errors reply `{"error":"<sentence>"}`, or plain text for a body or query that can't be read | `{"error":"<sentence>","code":"<code>"}` for both: match on `code` (`shot_not_running`, `shot_already_running`, `integration_not_found`, `request_invalid`, …), as the words may change; "no brew is running" now reads "no shot is running", and "a brew is already running" "a shot is already running" |
| `--brew-timeout-s` | `--shot-window-s` (the old name still works) |

For example, an automation that waits for `brew_started` now waits for
`shot_started`; one that checks `phase.phase == "brewing"` now checks
`shot.phase == "running"`; and one that reads `last_shot.volume_ml` now
reads `shot.last.weight_g`. One that only reacts to `shot_reported` and
`shot_aborted` keeps working, unless it reads the weight or
`in_grinder_range`.

### Security

- Web pages can no longer use gbs-anywhere through a browser in the home
  network. The app answers only to its IP address and home-network host
  names (`--allow-host` / `ALLOW_HOST` adds more, each by its full name;
  `*` alone allows any) and refuses changes requested from other sites,
  with an error like its others (`request_host_refused`,
  `request_from_other_site`). The grinder's routes refuse what a browser sends with its headers, which
  every change from a page does, and no longer let other sites read their
  replies. A page that re-points its own name to gbs-anywhere's address can
  still read the machine state the grinder polls, which holds nothing
  private. Requests to the grinder's routes are limited to 4 KiB. A refused
  request is logged once per device; should an E64 WS ever be refused,
  `--accept-browser-headers` lets it through.
- The E65/E80 pairing file is readable by its owner only, like the settings
  file and the shot history; any of them that others could read, say one
  from an earlier version, is made so when gbs-anywhere starts. Messages
  from the Half Decent Scale are limited in size.

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
  shot for up to the shot window (`--shot-window-s`, or the time set in the
  app; 3 minutes unless set); grinding again before pulling cancels the shot
  it was waiting for. A shot that cannot reach the grinder right away (say,
  the WiFi drops) is retried for two minutes. While the grinder is off or in
  standby, the log says so once rather than every minute. Tested with the
  E65W GbS.
- Settings in the app: the shot window (how long gbs-anywhere waits for a
  shot's numbers: 0:30 to 60:00 minutes or no limit, as in the API, with −
  and +, **No limit**, or typed as `2:30`, `150 s` or a number alone: a
  whole number is minutes up to 60 and seconds above, one with decimals is
  minutes) and the E65/E80 grinder's address (an IP address or name,
  optionally with `:port`). A new address takes effect at once and keeps
  the pairing. A setting changed in the app is saved in the settings file
  and wins over `--shot-window-s` or `--grinder` from then on
  (`PUT /api/settings`, `settings` in `GET /api/state`). The app shows
  where each value comes from (default, start settings, or set in the
  app), and **Use start setting** goes back to the flag or environment
  variable, else the default. In `PUT /api/settings`, `null` does the same
  for each setting, and `"grinder_host": ""` removes the address (without
  a start address, the same as `null`); `null` does not remove it.
  `settings` also carries the range the app can set (`shot_window_range`).
  The settings file keeps settings it does not know, saved by a newer
  version. Going back to an earlier version forgets the settings changed
  in the app.
- Integrations added in the app can be changed there; a password left empty
  keeps the saved one. An integration set with flags can be moved into the
  app, with all its settings, so nothing has to be typed again; its flags
  are ignored from the next start on while it is in the app, and can be
  removed (`GET /api/integrations/<id>/settings`,
  `PUT /api/integrations/<id>`, `POST /api/integrations/<id>/adopt`).
- Shot history: the last 200 shots the grinder asked for, with time, weight,
  target and how they ended (`outcome`: `succeeded`, `aborted` or
  `failed`), kept in `history.json` next to the settings file
  (`GET /api/history`). A shot is dated by the clock when it ends, and its length is counted on
  a clock that does not jump, so a computer that sets its clock from the
  network during a shot keeps both right. Until it syncs, a clock may still
  read the last shutdown time, or 1970 or the system's build date; shots
  dated before 2026 are listed last, under "Unknown day". When the file can
  no longer be saved, the app says the history is kept in memory for now.
- `grinder.shot_range` in `GET /api/state`: the shot times the grinder
  adjusts from for its current or last request. For the E64 WS that is
  over 10 s and up to 80 s; for an E65/E80 GbS, within 15 s of the brew
  job's target time and over 10 s (for a 28 s recipe: over 13 s, up to 43 s); `null`
  while it is not known. And `in_grinder_range` on each shot in
  `GET /api/history` that succeeded: whether the grinder adjusted from it,
  by that shot's window.
  The History tab shows a shot the grinder ignored with a grey **Ignored**
  badge instead of a green **Success**, and the app's hint while you type
  the time follows the same window.
- The app can show gbs-anywhere's log, in a card at the bottom of Status
  (also opened with **Log** next to the last error in System status): the
  last 1,000 lines it printed (INFO and up), and new ones as they come
  (`GET /api/log`, `GET /api/log/stream`). Copying or downloading it
  replaces IP addresses and passwords in addresses with placeholders, so it
  can go into a bug report.
- Update check: once a day gbs-anywhere asks GitHub for its latest release,
  so the app can say when a newer one is out (`update` in `GET /api/state`).
  GitHub sees the request like any other, including your IP address; it
  carries nothing but the gbs-anywhere version. Turn it off under Status →
  Settings (saved like the other settings; `update_check` in
  `PUT /api/settings`) or with `--update-check off` (`UPDATE_CHECK=off`).
- `grinder.wifi.pairing_id` in `GET /api/state`: the E65/E80 pairing in use;
  `grinder.wifi.unknown_pairing_id`: one typed in that the grinder did not
  take, while the app asks for the ID.
- `grinder.wifi.unreachable` in `GET /api/state`: true while an E65/E80 GbS
  does not answer at all (off, in standby or gone from the network), as
  opposed to answering with an error.
- When an E65/E80 GbS restarts during pairing and comes back without the
  pairing, the app says that the last pairing didn't stick and to accept
  again (`grinder.wifi.pairing_lost` in `GET /api/state`, until a pairing
  sticks). The README's "If pairing doesn't work" now says that a lost
  pairing can only be made anew and can take several tries.
- `server.boot_id` in `GET /api/state`: new on every start, so an app can
  tell that event and log numbers started over.
- `age_ms` on each event in `GET /api/events` and the event stream: how long
  ago it happened, so an app that catches up after a gap shows the right
  times.
- `cause` on `shot_aborted` events and on each shot in `GET /api/history`
  that was not a success: why it ended as a fixed code
  (`shot_aborted_by_user`, `shot_timed_out`, `grinder_shot_dropped`, …) for
  apps to match on, next to the English `reason`. Shots saved by earlier
  builds get theirs from `reason`.
- `detail_key` and `detail_vars` on each integration in `GET /api/state`:
  its status line as a key and values (such as `time_ms`, `weight_g`), next
  to the English `detail`. The app uses them, and its own catalog for the
  integrations' setup forms, to show both in German.
- A redesigned app with three tabs: Start (the three steps of a shot, with
  the step that is running or failed opened), Status (grinder, integrations,
  settings, version) and History. Appearance (System, Light, Dark) and
  language (English, Deutsch; by default the device's) can be set under
  Status → Settings; both are kept on each device. When no shot can start
  (no grinder connected, or the grinder blocked), System status opens by
  itself and says why in plain words, and step 1 shows "Blocked" instead of
  "Grind". When shots still work but something needs a look (an
  integration that is not working, or a grinder that turned a request
  down), System status turns yellow and says so. While an integration
  reports the shot, the fields to type it in are hidden; they come back,
  with a note, when every integration has failed or none has reported the
  shot a minute after it should have ended. The History tab replaces the
  list of this session's shots. An app left open reloads itself when
  gbs-anywhere is updated (once nothing is being typed in it), so it always
  matches the server.
- The image carries the licenses of the Rust crates built into it, in
  `/usr/share/doc/gbs-anywhere/THIRD-PARTY-NOTICES`.

### Changed

- The app tells a stop made on purpose from an error by `cause`, not by the
  English words, and a shot still open when the next one started gets its
  own title in the history.
- What the machine does (grind received, brew started, shot shown to the
  grinder) is now logged with a time and level like every other line, so it
  also shows in the app's log. The grinder's requests behind a grind and a
  brew start are logged at debug level only, as they say the same.
- The La Marzocco integration's log names the machine by the last four
  characters of its serial number, also in its errors, and gives
  La Marzocco's error replies by their status only (the reply itself at
  debug level). The card still shows the serial numbers in full.
- `--grinder` refuses an address that cannot be one (for example with a
  space in it) at startup, instead of failing on every request.
- A fractional `--shot-window-s` is rounded to whole seconds.
- `in_grinder_range` on `shot_reported` events is now also false for a
  shot of 10 s or less, which the E64 WS does not adjust from either (for
  an E65/E80 GbS, see `grinder.shot_range` above).
- The La Marzocco form calls its poll setting "Check interval (seconds)" and
  says what it does; the Half Decent Scale form asks for the scale's IP
  address.
- Unreleased builds are now published once a night as the `nightly` image
  tag, and only on nights when something changed. The `main` and `sha-*`
  tags are no longer updated; switch from `:main` to `:nightly`. `latest`
  and the version tags still come only from releases.

### Fixed

- gbs-anywhere stops at once on `docker stop` (SIGTERM) or Ctrl-C: it
  closes the app's live connections and lets open requests finish for up
  to 5 s, where Docker used to wait 10 s and then kill it.
- Requests on port 80 outside the grinder's `/api/v2/` paths, such as other
  devices on the network probing the port, no longer count as grinder
  traffic and are logged at debug level only, so they no longer fill the
  app's log. The `--log` transcript still records them.
- A coffee log that the La Marzocco cloud sends with its list as `null`, or
  without the list, now counts as empty, instead of logging a warning about
  an unexpected shape and showing that error on the integration's card.

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
