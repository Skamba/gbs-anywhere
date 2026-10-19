---
description: Prepare a gbs-anywhere release X.Y.Z (version bump and CHANGELOG in a pull request, then the tag after merging). Only when asked for a release.
argument-hint: X.Y.Z
disable-model-invocation: true
---

Prepare release `$ARGUMENTS` of gbs-anywhere, following "Git and releases"
in AGENTS.md. A pushed `vX.Y.Z` tag publishes the `latest` image and the
GitHub release, and release tags cannot be moved or deleted, so stop and ask
before every push and before the tag.

## 1. Check before starting

- `$ARGUMENTS` is `X.Y.Z` (or `X.Y.Z-rc.N` for a prerelease), without a
  leading `v`. Without one, ask for it.
- `git fetch origin`; the tag `v$ARGUMENTS` exists neither locally nor on
  `origin` (`git ls-remote --tags origin`).
- The version is higher than the one in Cargo.toml on `origin/main`.
- `## [Unreleased]` in CHANGELOG.md has entries.
- No open issue lists an unchecked item marked "before the release"
  (`gh issue list --state open`, then read the ones that track follow-ups).
  If one does, name it and stop.

## 2. The release pull request

- Branch `release/v$ARGUMENTS` from `origin/main`.
- Set `version` in Cargo.toml and run `cargo build` so Cargo.lock follows.
- In CHANGELOG.md, rename `## [Unreleased]` to `## [$ARGUMENTS] - <today,
  YYYY-MM-DD>` and put a fresh, empty `## [Unreleased]` above it. The
  release notes are that section, as CI cuts it out, so read it once as a
  person upgrading would: breaking changes first and complete.
- Look through README.md for wording tied to the old version, such as a
  feature said to be only in the `nightly` image.
- Run the checks from "Done means" in AGENTS.md.
- Commit as `Release $ARGUMENTS`, with a body that names the main changes.
- Ask before pushing the branch and opening the pull request.

## 3. After the pull request is merged

Only once the person says it is merged:

- `git fetch origin` and find the merge commit on `origin/main` that holds
  the version bump.
- Ask before tagging. Then `git tag v$ARGUMENTS <that commit>` and
  `git push origin v$ARGUMENTS` (that one tag, never `--tags`).
- `image.yml` checks the tag against Cargo.toml and CHANGELOG.md, builds
  the image, and creates the GitHub release. Give the person the link to
  that workflow run.
