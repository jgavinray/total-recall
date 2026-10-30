# Changelog

## [0.1.1] - 2026-10-30

### Bug Fixes

- Session ids are unique per server instance, so concurrent signoff appends
  can no longer collide on a stamp when instances start in the same
  nanosecond.
- The harness-config tests compare the canonicalized binary path, so they hold
  on macOS, where the build path crosses the `/tmp` symlink.
- The timezone-change refusal test drives both startups through the documented
  `EXO_TZ` override, so it holds on UTC-configured machines.

### Continuous Integration

- GitHub Actions build and test on Linux and macOS, lint with rustfmt and
  clippy, check Conventional Commits on every new commit, and keep the
  SHA-pinned actions updated through Dependabot.
- Releases are tagged and published on demand through the manual Release
  workflow (`.github/workflows/release.yml`).
