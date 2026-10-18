# Contributing

Thanks for helping. A few things make a contribution easy to review and merge.

## Before you start

- **Questions and ideas** go in
  [Discussions](https://github.com/Skamba/gbs-anywhere/discussions).
- **Bugs** and **reports from your own setup** go in
  [Issues](https://github.com/Skamba/gbs-anywhere/issues/new/choose).
- **For anything bigger than a small fix**, open an issue or discussion first,
  so we can agree on the approach before you write the code.

## Pull requests

- One change per pull request. A new integration, a layout change and a
  translation are three pull requests; small ones are reviewed much faster.
- Say what you tested it on: which grinder, machine, scale or integration,
  or "tests only".
- `cargo fmt --check`, `cargo clippy --all-targets` (no warnings) and
  `cargo test` must pass, and clippy and the tests also with
  `--features bluetooth`. CI runs them on every pull request.
- Update README.md when behaviour, flags or the API change, and add a line
  under `## [Unreleased]` in CHANGELOG.md for anything users will notice.
- The repository is public: leave out IP addresses, serial numbers, account
  names and logs from real devices.

[AGENTS.md](AGENTS.md) has the full conventions: the code layout, how to run
it locally, and the rules for dependencies and wording.

## Adding an integration

Each integration is one folder under `src/integration/`; `la_marzocco/` is a
complete example, and the rules are in the
[`integration` module docs](src/integration/mod.rs). Test against a local fake
of the vendor's service, not the real one. Its texts are English in the code,
with their German in the app's catalog (see AGENTS.md); `cargo test` checks
that none is missing. If German is not your language, say so in the pull
request and ask for help.

## License

By contributing you agree that your work is licensed under the
[AGPL-3.0-or-later](LICENSE), like the rest of the project.
