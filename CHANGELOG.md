# Changelog

Notable changes for people running gbs-anywhere, newest first. Versions follow
[Semantic Versioning](https://semver.org/); while the version starts with 0,
a minor release may change behaviour.

## [Unreleased]

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
