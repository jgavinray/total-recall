# Release management workflow — proposal and implemented design

Status: **reviewed and approved by the owner 2026-10-27.** The implemented design
is the manual workflow in the section of that name below; the release-please
proposal that follows it is the 2026-10-24 history, kept for the record — it was
built, blocked on an owner-only repository setting, and superseded by the
owner's ruling that releases are tagged manually, on demand. Written 2026-10-24.

## Goal

The owner's request, as it evolved:

- 2026-10-24: "a workflow that will open a pull request and continually add
  commits to it for release management/tagging."
- 2026-10-27, final: releases are **tagged manually, on demand** — the owner
  controls the version and sees the full release notes before anything is
  published.

## Implemented design (2026-10-27)

`.github/workflows/release.yml`, `workflow_dispatch` only — nothing runs on a
push, nothing opens pull requests, no third-party release tool is involved, and
the repository setting "Allow GitHub Actions to create and approve pull
requests" is irrelevant to releases.

Inputs at dispatch:

- `version` (required): semver; must EQUAL the `version` in `Cargo.toml`. The
  bump itself — `Cargo.toml`, `Cargo.lock` (CI's `--locked` enforces the sync),
  and an optional `CHANGELOG.md` section — lands as a normal pull request
  first. A version that is already tagged is refused, so nothing is released
  twice.
- `notes` (optional): pasted markdown wins; otherwise the `## [X.Y.Z]` section
  of `CHANGELOG.md`; otherwise notes built from the commits since the last tag,
  grouped Features / Bug Fixes / Other Changes.
- `publish` (default off): a dry run builds the notes, prints them into the
  run summary, and tags nothing. Checked, it runs
  `gh release create vX.Y.Z --target <run commit>` with those notes — tag and
  GitHub release in one step, using only the built-in token's
  `contents: write`.

The Homebrew formula (`jgavinray/homebrew-tap`, `Formula/total-recall.rb`)
stays a manual follow-up: each release needs its `url` and `sha256` updated to
the new tag's tarball.

Verified 2026-10-27 by running the workflow file itself through the GitHub
Actions runner simulator (`act`) against a sandbox clone: the dry run builds
the notes, prints them into the run summary, and tags nothing; a version
mismatch and an already-tagged version each fail with their exact `::error::`
message; `publish` gates the tag/release step, which fires only when checked
(the simulated image lacked `gh`; GitHub's runners ship it). The dispatch
itself is owner-triggered from the Actions UI.

---

# The 2026-10-24 proposal (history; superseded)

## Facts this proposal rests on (checked 2026-10-24)

- `Cargo.toml` is at `0.1.0`. Tags `v0.0.1` (d82feb4) and `v0.1.0` (658b263) exist;
  `main` is 5 commits past `v0.1.0`. There is no `CHANGELOG.md`.
- GitHub has a release for `v0.0.1` only; the `v0.1.0` tag has no release.
- The `total-recall` crate name on crates.io belongs to another project
  (stepanov-denis/total-recall, 0.3.0, last updated 2022). This crate cannot be
  published there under its own name, and any tool that reads the current version
  from crates.io would see that crate's `0.3.0`.
- The Homebrew formula (`jgavinray/homebrew-tap`, `Formula/total-recall.rb`) builds
  from the source tarball of a tag: `url .../archive/refs/tags/v0.1.0.tar.gz` plus
  its `sha256`. Each release needs those two lines updated in the tap.
- Commit messages are checked against Conventional Commits from 2026-10-24
  (`.github/workflows/commits.yml`). Of the 29 commits before that, 2 conform;
  release tools ignore non-conforming commits when they compute versions and
  changelogs.
- `main` takes direct pushes; it does not require pull requests.

## Options from the open-source community

| Tool | Release PR | Rust-aware (`Cargo.toml`, `Cargo.lock`) | Reads crates.io | Runs as | Activity / license |
|---|---|---|---|---|---|
| **release-please** (googleapis/release-please-action) | Yes, kept up to date as `main` moves | Yes (`release-type: rust`) | No: git tags, GitHub releases and a manifest file | One JavaScript action | 2.6k stars, pushed 2026-08-28, Apache-2.0 |
| **release-plz** (release-plz/release-plz, release-plz/action) | Yes | Yes, Rust-native; changelog via git-cliff; optional cargo-semver-checks | Yes by default, to find the last published version | Composite action that adds taiki-e/install-action, cargo-bins/cargo-binstall and release-plz/git-config, and downloads prebuilt binaries | 1.5k stars, pushed 2026-10-24, Apache-2.0 |
| **knope** (knope-dev/knope) | Yes, via its workflow config | Yes | No | Rust binary | 200 stars, pushed 2026-10-23, MIT |
| **cocogitto** (`cog bump`) | No built-in PR flow; bumps and tags directly | Via hooks | No | Rust binary / action | cocogitto-action last pushed 2026-03-04 |
| **semantic-release** | No: releases on every push, no PR | Via plugins | Via plugins | Node | — |

Only release-please and release-plz provide the requested standing release PR out of
the box. knope can, with more configuration.

## Recommendation: release-please

1. **It never consults crates.io.** The current version comes from a manifest
   file in the repo. That makes the name collision above harmless. release-plz
   would need `publish = false` and a check that it then finds the version
   from git tags instead of the other project's crate. Its docs don't say it
   does (open item V1).
2. **Smallest supply chain.** One action, pinned to a commit SHA, and no binary
   downloads. release-plz's action brings in three more third-party actions plus
   prebuilt binaries.
3. **It updates the Rust files.** `release-type: rust` bumps the version in
   `Cargo.toml` and `Cargo.lock` and writes `CHANGELOG.md`.

release-plz is the better fit if the crate is ever renamed and published to
crates.io, because cargo-semver-checks catches unflagged API breaks.

## Proposed design

### Files

- `.github/workflows/release.yml`: on push to `main`, one job runs
  `googleapis/release-please-action` (SHA-pinned). It opens or updates the
  release PR. When the release PR has just been merged, it creates the tag and
  GitHub release.
- `release-please-config.json`: one package at `.`, `release-type: rust`,
  `include-component-in-tag: false` (tags stay `vX.Y.Z`, matching the existing
  tags and the formula URL), `bump-minor-pre-major: true` (a breaking change in
  0.x bumps the minor, not to 1.0.0). Changelog sections: `feat` → Features,
  `fix` → Bug Fixes, `perf` → Performance; other types hidden.
- `.release-please-manifest.json`: `{".": "0.1.0"}`, plus
  `last-release-sha` / `bootstrap-sha` at the `v0.1.0` commit, because
  `v0.1.0` has no GitHub release for the tool to find (open item V2).

### Version rules (Conventional Commits, while the version is 0.x)

- `fix:` → patch (0.1.0 → 0.1.1)
- `feat:` → minor (0.1.0 → 0.2.0); with `bump-patch-for-minor-pre-major` it
  would be a patch instead (owner's choice, question Q4).
- `feat!:` / `BREAKING CHANGE:` → minor while 0.x (0.1.0 → 0.2.0)
- `docs:`, `ci:`, `chore:`, `test:`, `refactor:`, `style:`, `build:` → no release
  on their own.

### How the release PR behaves

- Each push to `main` with releasable commits updates the one release PR (title
  `chore(main): release X.Y.Z`): version, `Cargo.lock`, changelog.
- **Difference from the request:** release-please regenerates the release
  branch from `main` on each run. It doesn't stack commit after commit on top.
  The PR always shows the release as of the current `main`, which matches the
  intent ("continually" up to date), but its history is one regenerated commit
  (open item V3, to confirm in a sandbox).
- Merging the release PR (merge or squash) lands the version bump. The next run
  creates tag `vX.Y.Z` and a GitHub release whose notes are the changelog
  section. Squash gives `chore(main): release X.Y.Z (#N)`, which passes the commit
  check; a merge commit is exempt from it.

### Homebrew tap update (optional second job)

After a release is created, a job downloads the tag tarball, computes its
`sha256`, and opens a pull request on `jgavinray/homebrew-tap` changing `url`
and `sha256` in `Formula/total-recall.rb`. Candidates:
`mislav/bump-homebrew-formula-action` (SHA-pinned), or about 20 lines of
script with `curl` and `git`. Either needs a token with write access to the tap.

### Tokens and repository settings (owner-only)

- With the built-in `GITHUB_TOKEN`, the release PR gets **no CI runs**: GitHub
  does not trigger workflows from events that token creates. CI still runs on
  `main` after the merge. To get CI on the release PR, the owner provides a
  fine-grained personal access token or a GitHub App token, scoped to this repo
  (contents, pull requests), as a repository secret.
- Repository setting: "Allow GitHub Actions to create and approve pull
  requests" must be on.
- Homebrew job: a second token with contents and pull-request write on
  `jgavinray/homebrew-tap` only.

### Permissions in the workflow

`contents: write` and `pull-requests: write` on the release job only; every
other workflow stays `contents: read`.

## To verify in a sandbox repository before implementing

- **V1.** (Only if release-plz is chosen) With `publish = false`, release-plz
  takes the last version from git tags and never from crates.io.
- **V2.** release-please with the manifest and `last-release-sha` at `v0.1.0`
  proposes the right next version and changelog, despite `v0.1.0` having no
  GitHub release and the history since `v0.1.0` being non-conventional.
- **V3.** How the release PR branch is updated: regenerated or appended.
- **V4.** The exact tag format with `include-component-in-tag: false` is
  `vX.Y.Z`.

## Questions for the owner

- **Q1.** release-please (recommended) or release-plz?
- **Q2.** Token: built-in `GITHUB_TOKEN` (no CI on the release PR) or a token
  you create (CI runs on the release PR)?
- **Q3.** Automate the Homebrew formula bump (needs a tap token), or keep
  updating the formula by hand?
- **Q4.** While at 0.x, should `feat:` bump the minor (0.1.0 → 0.2.0) or the
  patch?
- **Q5.** Create the missing GitHub release for `v0.1.0` first (it publishes a
  release page), or bootstrap with `last-release-sha` only?
- **Q6.** Prebuilt binaries attached to releases (for example via cargo-dist), or
  source-only as today (Homebrew builds from source)?

## Noticed, out of scope

- `Cargo.toml` says `license = "GPL-2.0"` (a deprecated SPDX id); the formula says
  `GPL-2.0-only`. The two should agree.
