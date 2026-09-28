# AGENTS.md

gbs-anywhere stands in for the espresso machine a Mahlkönig E64 WS grinder
syncs with (Grind-by-Sync). The grinder polls it over HTTP on port 80; a
person on their phone, or an optional integration, reports each shot's time
and weight; the grinder then adjusts its grind setting. One Rust crate, one
HTML page, one Docker image. README.md is for users; this file is for agents
changing the code.

## Layout

- `src/protocol/`: the grinder link as plain types, no I/O. `machine.rs` holds
  the state machine and shot rules, `wire.rs` the JSON field names.
- `src/server/`: axum. `grinder_api.rs` is what the grinder calls
  (`/api/v2/*`); `control_api.rs` is the JSON API and page for people.
- `src/integration/`: optional integrations that report shots by themselves,
  one folder each (see [Integrations](#integrations)).
- `web/index.html`: the whole phone app (HTML, CSS, JS), no build step.
- `src/main.rs`: flags and startup only.

## Commands

```sh
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
# Local run, then open http://localhost:8089/
cargo run -- -p 8089 --control off --no-stdin --config target/dev.json
```

- `web/index.html` is compiled into the binary (`include_str!`). After
  editing it, rebuild and restart the server; refreshing the browser alone
  shows the old page.
- Port 80, the default, is what a real grinder uses. Use another port when 80
  is taken. Keep local settings files under `target/` (git-ignored), because
  they hold passwords.
- Docker is not needed for development; CI builds the image.

## Done means

- `cargo fmt --check`, `cargo clippy --all-targets` (no warnings) and
  `cargo test` pass. CI (`.github/workflows/ci.yml`) runs the same checks on
  every push and pull request.
- For page changes, open the page at phone width (about 390 px) in light and
  dark mode and look at the result. There is no JavaScript test suite.
- README.md (and an integration's own README) matches any change to
  user-visible behaviour, flags or the API.

## Code conventions

- Rust 2024; the minimum Rust version is `rust-version` in Cargo.toml.
  `unsafe` is forbidden by a lint.
- Match the surrounding code: short doc comments saying what and why,
  `anyhow` with `.context(...)` for errors people will read, and messages in
  plain sentences.
- Keep `src/protocol/` free of I/O and clocks (callers pass `Instant`s), so
  the grinder's behaviour stays testable without a grinder.
- The grinder-facing API (`/api/v2/*`, names in `wire.rs`) is exactly what
  the grinder expects. Don't rename, drop or retype fields there.
- Few dependencies, all pure Rust. TLS is rustls with the `ring` provider (no
  OpenSSL, no cmake), which keeps the distroless image working. Give the
  reason when adding a dependency.
- Tests sit next to the code in `#[cfg(test)] mod tests` and go through real
  functions rather than mocks. Temporary files go under
  `std::env::temp_dir()`. `src/server/tests.rs` runs the real routers over
  HTTP with `GrinderModel` playing the grinder; extend it when what the
  grinder sees changes.

## Integrations

Each integration is a folder `src/integration/<id>/` with `mod.rs`
(`pub static KIND: Kind`), `config.rs` (the form's `FIELDS`, a typed config,
and a clap `Args` with `#[group(id = "<id>")]`), `run.rs` (implements
`Integration`), `icon.svg` and `README.md`. Register it in
`src/integration/mod.rs` in four places: `pub mod`, `KINDS`, a `CliArgs`
field and its line in `CliArgs::configured`.
The module docs there are the full spec; `la_marzocco/` is the worked example.

- Report shots only through `Link::report`. It holds the rules every
  integration shares: a shot counts only while the grinder waits after a knob
  press, and a missing weight falls back to the recipe weight.
- Secrets are `Input::Password` fields. Settings never go back out through
  the API or into logs. Keys that are not on the form, such as a test base
  URL, stay command-line only.
- Vendor logos are trademarks: draw a generic glyph for `icon.svg`. People
  can supply their own icons with `--icons`.
- Test against a local fake of the vendor's service. Don't send test traffic
  or made-up credentials to a vendor's real service.

## Web app

- One file, no framework, no requests to other hosts: it is served on a home
  network that may have no internet access.
- Colours are CSS variables on `:root`, redefined for dark mode. Use them
  instead of literal colours.
- Phone first: content at most 480 px wide, 16 px side padding, no
  horizontal scrolling.
- Typing the time and weight by hand is the default flow and must work with no
  integration set up. Integrations are optional, behind the green + in the
  header.

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
  job of `ci.yml` must pass. A push to `main` publishes the `main` image tag;
  a `vX.Y.Z` tag publishes `latest` and creates the GitHub release. Ask
  before pushing, force-pushing, tagging or changing `.github/workflows/`.
- User-visible changes get a line under `## [Unreleased]` in CHANGELOG.md.
- To release: in a pull request, set `version` in Cargo.toml and rename
  `[Unreleased]` to `[X.Y.Z] - <date>` with a fresh `[Unreleased]` above it;
  after merging, tag that commit `vX.Y.Z` and push the tag. CI stops the
  release if the tag, Cargo.toml and CHANGELOG.md disagree. Release tags
  cannot be moved or deleted.
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
