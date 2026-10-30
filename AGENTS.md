# AGENTS.md

gbs-anywhere stands in for the espresso machine a Mahlkönig grinder syncs
with (Grind-by-Sync). The E64 WS polls it over HTTP on port 80; an E65/E80
GbS is polled by it instead, over the grinder's Local WiFi HTTPS API
(`--grinder`). A person on their phone, or an optional integration, reports
each shot's time and weight; the grinder then adjusts its grind setting. One
Rust crate, one HTML page, one Docker image. README.md is for users; this
file is for agents changing the code.

## Layout

- `src/protocol/`: the grinder link as plain types, no I/O. `machine.rs` holds
  the state machine and shot rules, `wire.rs` the JSON field names, `wifi.rs`
  the E65/E80 Local WiFi messages, and `grinder.rs` models how the E64 WS
  reads the machine's replies (`GrinderModel`, used in tests).
- `src/server/`: axum. `grinder_api.rs` is what the E64 WS calls
  (`/api/v2/*`); `wifi_grinder.rs` pairs with an E65/E80 GbS and turns its
  brew jobs into shots of the same machine, with its parts in
  `wifi_grinder/` (`pair.rs` the pairing, `jobs.rs` the brew jobs,
  `slot.rs` the one place that starts and stops the link);
  `control_api.rs` is the JSON API for people and `page.rs` builds the app
  page. `browser.rs` keeps web pages out: the app answers only to
  home-network host names, refuses writes from other sites, and the
  grinder API refuses requests with browser headers. Every control route
  gets these checks through `route_layer`, so a new route needs nothing
  extra. `mod.rs` builds the server and its listeners. Around them:
  `settings.rs` holds the settings people change in the app (shot window,
  grinder address, which restarts the E65/E80 link, update check) and is
  the only writer of the settings file; `history.rs` records the shots
  from the machine's events; `logbook.rs` keeps the last log lines for the
  app (a `tracing` layer next to the console output); `update_check.rs`
  looks for a newer release; `private_file.rs` writes the files only their
  owner may read (settings, history, pairing, transcript); `utc.rs` formats
  times. `tests.rs` and `wifi_grinder/tests.rs` (by topic in
  `wifi_grinder/tests/`) hold the end-to-end tests.
- `src/integration/`: optional integrations that report shots by themselves,
  one folder each (see [Integrations](#integrations)). `mod.rs` has the
  shared rules (`Link`), `kind.rs` the setup forms and `manager.rs` the
  running integrations, saved through `server/settings.rs`.
- `web/`: the phone app, one page with no build step. `index.html` holds the
  markup and the small script that sets the theme before the first paint,
  `app.css` the styles, `app.js` the app's script, and `strings/<lang>.json`
  its texts, one file per language. At startup `server/page.rs` fills the
  texts and the version (`PAGE_VERSION`) into `app.js`, then the styles and
  the script into `index.html`, so the page still comes in one response.
- `src/main.rs`: flags and startup only. `src/lib.rs`: the crate root and
  `version()`.
- `packaging/`: the release builds besides the image. `build-release.sh`
  makes the binary tarball and the Debian package (cargo-deb, configured in
  Cargo.toml); `deb/` holds the package's systemd unit, its
  `/etc/default/gbs-anywhere` and `postrm`. The unit's `ExecStart` and the
  Dockerfile's `ENTRYPOINT` pass the same flags: change them together. The
  Dockerfile shares `install-cargo-about.sh`.

## Commands

```sh
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
# Local run, then open http://localhost:8089/
cargo run -- -p 8089 --control off --no-stdin --config target/dev.json --update-check off
```

- The files in `web/` are compiled into the binary (`include_str!`). After
  editing one, rebuild and restart the server; refreshing the browser alone
  shows the old page.
- Port 80, the default, is what a real grinder uses. Use another port when 80
  is taken. Keep local settings files under `target/` (git-ignored), because
  they hold passwords.
- `.claude/launch.json` starts the same local run for an agent's browser
  preview. Project skills in `.claude/skills/`: `/check-page` walks through
  the page checks below, `/release X.Y.Z` the release steps.
- Docker is not needed for development; CI builds the image.

## Done means

- These pass, as CI (`.github/workflows/ci.yml`) runs them on every push and
  pull request: `cargo fmt --all --check`,
  `cargo clippy --all-targets --locked -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked`,
  `cargo test --locked`, and the build with the minimum Rust version (see
  [Code conventions](#code-conventions)).
- For page changes, open the page at phone width (about 390 px) in light and
  dark mode and look at the result. There is no JavaScript test suite, so
  also check by hand what reviews keep finding:
  - **After a gap:** a reload, a phone that slept, a tab in the background, a
    server restart. Times come from the server (`age_ms`), never from when
    the page received an event, and an old error doesn't come back after a
    reload.
  - **Requests and live events:** mark something as the person's only after
    the server accepted it; its event can arrive before the reply.
  - **Live regions:** write a `role="status"` region only when its text
    changes, so screen readers don't repeat it.
  - **Every state:** a rule for one state (say, showing "Blocked") holds in
    every state it belongs to, also after a shot was done, aborted or failed.
  - **Deliberate behaviour stays:** keep what the page does on purpose (a
    typed weight stays for the next shot) unless the change says why.
  - **Values, not wording:** match machine-readable values from the server
    (codes, limits in `/api/state`), not its English text.
- README.md (and an integration's own README) matches any change to
  user-visible behaviour, flags or the API.

## Code conventions

- Rust 2024; the minimum Rust version is `rust-version` in Cargo.toml.
  Don't use std or language features newer than that version: the `msrv`
  job in CI runs `cargo check --all-targets --locked` with it (locally:
  `cargo +<version> check --all-targets --locked`), and everything else
  builds with the latest stable. `unsafe` is forbidden by a lint.
- Match the surrounding code: short doc comments saying what and why,
  `anyhow` with `.context(...)` for errors people will read, and messages in
  plain sentences.
- Items are `pub(crate)` or private; only what `src/main.rs` uses is `pub`.
  That way the compiler reports code nothing uses: remove it, or give it
  `#[cfg(test)]` when only tests need it. A public doc comment links only
  to public items (`cargo doc` checks it in CI); name the others in plain
  backticks.
- Keep `src/protocol/` free of I/O and clocks (callers pass `Instant`s), so
  the grinder's behaviour stays testable without a grinder.
- The grinder-facing API (`/api/v2/*`, names in `wire.rs`) is exactly what
  the grinder expects. Don't rename, drop or retype fields there. The same
  goes for the E65/E80 messages in `wifi.rs`, which also must keep their
  header spelling (`http1_title_case_headers`).
- Few dependencies, all pure Rust. TLS is rustls with the `ring` provider (no
  OpenSSL, no cmake), which keeps the distroless image working. Give the
  reason when adding a dependency.
- Tests sit next to the code in `#[cfg(test)] mod tests` and go through real
  functions rather than mocks. Temporary files go under
  `std::env::temp_dir()`, through `temp_dir(name)` or `temp_path(name)` in
  `src/test_support.rs`. `src/server/tests.rs` runs the real routers over
  HTTP with `GrinderModel` playing the grinder; extend it when what the
  grinder sees changes.

## API and saved files

- The control API's names follow the rules at the top of
  `src/server/control_api.rs`: snake_case; an event or cause is
  `<group>_<object>_<past participle>` (`grinder_shot_requested`,
  `shot_aborted`); a state is one word (`running`, `connected`); units are
  in the name (`_ms` measured, `_s` set by people, `_g`); a point in time
  ends in `_at`, an age in `age_ms`. `reason`, `detail` and `error` are
  English for people; codes go in `cause`, `kind`, `state`, `phase` and
  `code`, and every error has a `code`. The route table there is the API
  reference (README links to it): change it with the route.
- Renaming or dropping a name in the API, a flag or a saved file is a
  breaking change: list it under `### Changed (breaking)` in CHANGELOG.md,
  old name next to new. Keep old input working where that is cheap, with
  serde or clap `alias` (as `--brew-timeout-s`).
- The settings file, `history.json` and the E65/E80 pairing file carry no
  version. A newer build reads an older file through `#[serde(default)]` on
  new fields and `serde(alias)` on old names; an older build reading a newer
  file keeps settings keys it doesn't know (`other`) and drops a shot it
  can't read, not the whole history. Add a test that loads the file as an
  older version wrote it.
- `AbortCause::text` is saved too: older history entries get their `cause`
  back from it (`cause_of` in `history.rs`). Rewording one means adding its
  old words there.
- A new `AbortCause` needs its row in `CAUSES` in `app.js`; a new
  `ShotOutcome` a `.k-<code>` rule in `app.css` and `history.kind.<code>` in
  the catalogs. Tests in `page.rs` check both.
- Lengths are measured on `Instant`. The wall clock only dates things, and
  may be unset (1970) or jump.
- The page ships with its server, so it has no branches for older servers.
  The one exception is reading `version` in `refresh()`: when it changes,
  the page reloads to get the new server's page.
- Anything a request or a peer can make the server keep (log lines, refused
  hosts, shots) has a size limit. A log line never offers a ready-made flag
  for a value someone else chose, such as a refused host name.

## Integrations

Each integration is a folder `src/integration/<id>/` with `mod.rs`
(`pub(crate) static KIND: Kind`), `config.rs` (the form's `FIELDS`, a typed
config, and a clap `Args` with `#[group(id = "<id>")]`), `run.rs`
(implements `Integration`), `icon.svg` and `README.md`, plus more files as
it needs them (`la_marzocco/cloud.rs`, `half_decent/shot.rs`). Register it
in `src/integration/mod.rs` in four places: `pub(crate) mod`, `KINDS`, a
`CliArgs` field and its line in `CliArgs::configured`.
The module docs there are the full spec; `la_marzocco/` is the worked example.

- Report shots only through `Link::report`. It holds the rules every
  integration shares: a shot counts only while the grinder waits after a knob
  press (on an E65/E80 GbS: after a Grind-by-Sync grind), and a missing
  weight falls back to the recipe weight.
- Secrets are `Input::Password` fields. Passwords never go back out through
  the API or into logs. The other form values come back for editing
  (`GET /api/integrations/<id>/settings`, where `secrets_set` only names the
  passwords that are set). Keys that are not on the form, such as a test
  base URL, stay command-line only. Leave such a key out of `Args::settings`
  while it has its default (as `la_marzocco` does with `base_url`): an
  integration whose settings hold one can't be moved into the app.
- Vendor logos are trademarks: draw a generic glyph for `icon.svg`. People
  can supply their own icons with `--icons`.
- Texts stay English in Rust. Give each status line a key with
  `Detail::keyed` (`<id>.<state>`) and its numbers with `Detail::var`
  (unit in the name: `age_ms`, `time_ms`, `weight_g`); the API shows them as
  `detail_key` and `detail_vars` next to `detail`. The German wording goes
  into `web/strings/de.json`, `integ.detail.<key>` for status lines and
  `kind.<id>.summary` / `kind.<id>.field.<key>.label|help|placeholder` for
  the form. The placeholder leaves the unit out (`time_ms` is `{time}`);
  `age_ms`, `time_ms` and `weight_g` get their own format, other numbers up
  to one decimal. Another unit or format needs a case in `integDetail()`.
  `cargo test` checks that the form and every keyed status line of each
  integration have their German; at run time a missing entry shows the
  server's English. Errors and log lines stay English.
- Test against a local fake of the vendor's service. Don't send test traffic
  or made-up credentials to a vendor's real service. The same goes for
  GitHub in the update check: tests pass a local URL (`UpdateConfig`).

## Web app

- One page, no framework, no build step, no requests to other hosts: it is
  served on a home network that may have no internet access. The server
  fills the styles, the script and the texts into the page (see
  [Layout](#layout)), so it still comes in one response.
- Colours are CSS variables on `:root`, redefined for dark mode. Use them
  instead of literal colours.
- Every text the app writes is in the catalogs in `web/strings/` (`STRINGS`
  in the script), in `en.json` and `de.json` with the same keys, and goes
  through `t()`. A plural is an object of forms, `{"one": …, "other": …}`.
  Where the E64 WS and an E65/E80 need different words, `tv(key)` picks
  `<key>.e64` or `<key>.wifi` for the grinder in use; give both.
  Integration texts that come from the server in English (`kind.*`,
  `integ.detail.*`) are the exception: they are only in `de.json`, and
  English shows the server's text. The tests in `page.rs` read the
  catalogs as JSON and check this: the same keys and placeholders in every
  language, both `.e64` and `.wifi` wordings, and every key the page passes
  literally to `t()`/`tv()` or names in `data-i18n` is in the catalog.
- A new language is a file in `web/strings/`, its line in `LANGS` in
  `page.rs` (the page takes its list from there), its button under
  Status → Settings → Language, and its fallback locale in `setLanguage()`.
- Phone first: content at most 480 px wide, 16 px side padding, no
  horizontal scrolling.
- Typing the time and weight by hand is the default flow and must work with no
  integration set up. Integrations are optional, behind the + under
  Status → Machine integrations.

## Public repository wording

The repository and its history are public. In code, comments, docs, commit
messages and pull requests:

- Write in English.
- Describe what the app does for the person using it. Don't write about how
  the grinder's protocol was worked out, and don't refer to vendor software
  internals or vendor-internal tools.
- Leave out anyone's local setup: IP addresses (except documentation
  examples such as `192.168.1.20`), hostnames, serial numbers, account names,
  photos, and logs or transcripts from real devices.
- Name vendors only to say what works with what, and keep the "Not
  affiliated with Mahlkönig or Hemro" note.

## Git and releases

- Work on a branch. `main` is what users run.
- `main` is protected: changes go in through a pull request, and the `test`
  job of `ci.yml` must pass. Each night `image.yml` publishes `main` as the
  `nightly` image tag, unless `main` has not changed since the last one; a
  `vX.Y.Z` tag publishes `latest` and creates the GitHub release, with
  the tarballs and Debian packages attached. gbs-anywhere-pi looks for a new
  release each day and builds its Raspberry Pi image by itself. Ask
  before pushing, force-pushing, tagging or changing `.github/workflows/`.
- User-visible changes get a line under `## [Unreleased]` in CHANGELOG.md.
- To release: in a pull request, set `version` in Cargo.toml and rename
  `[Unreleased]` to `[X.Y.Z] - <date>` with a fresh `[Unreleased]` above it;
  after merging, tag that commit `vX.Y.Z` and push the tag. CI stops the
  release if the tag, Cargo.toml and CHANGELOG.md disagree. Release tags
  cannot be moved or deleted.
- Every release updates the [wiki](https://github.com/Skamba/gbs-anywhere/wiki)
  (its own repository, `gbs-anywhere.wiki`): once the release is out, bring
  the pages its changes touch in line with it. README.md links there for new
  users, so the wiki describes the latest release, not `main`.
- Commit messages have an imperative subject line of at most about 70
  characters and a body that explains why.

## How to work here

Do what was asked, at the scope asked. Make routine decisions yourself and
mention them in your summary. Stop and ask only when two readings of the
request would lead to different work, or before anything destructive or
public, such as pushing, deleting data or publishing an image.

## License

AGPL-3.0-or-later (`LICENSE`). Source files carry no license header. Only
add third-party code, assets or dependencies whose license is compatible with
the AGPL (MIT, Apache-2.0, BSD and ISC are); ask when unsure.

The image ships the crates' licenses (`THIRD-PARTY-NOTICES`, made by
cargo-about from `about.toml` and `about.hbs`). A crate under a license not
in `accepted` there fails the image build: add the license only once it is
known to be compatible. `cargo about generate about.hbs` checks it locally.
