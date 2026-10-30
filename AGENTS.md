# AGENTS.md — working ON total-recall

Orientation for an agent editing this software (not using it — for that, read
`README.md` and `spec.md`). These rules are intrinsic to the repository and
hold on any checkout. Workstation-specific conventions — the model fleet, the
memory store, harness roles, GPG settings — live in the global agent
configuration, not here.

## Spec-first discipline

- `spec.md` is the binding contract. Code descriptions are its model-facing
  superset; pinned refusal texts and `inputSchema`s are byte-contracts — NEVER
  paraphrase one in code, and never change one to make a test pass.
- Schemas are FROZEN: an `inputSchema` or pinned refusal-text change requires a
  spec amendment (a maintainer-authorized `spec.md` edit) first, not the other
  way around. The `initialize` `instructions` field is spec §11 verbatim.
- Navigate and edit with the LSP/ast tooling before any `sed`-style rewrite.

## Ownership map

- `src/lib.rs` and `src/tools/mod.rs` are the registries every module wires
  through. Treat them as single-writer files: change them in one place at a
  time, and add tools by creating module files rather than editing the
  registries in parallel (their headers say so).
- `spec.md` changes need the maintainer's authorization (spec amendment).
- `tests/gate_w1.rs`..`gate_w4.rs` and `tests/rpc_tests.rs` hold the contract
  pins (refusal texts, envelope shapes, §11 instructions line-by-line).
  Touching a pin needs stated justification in the commit message or pull
  request — a re-pinned assertion is a spec event, not a test fix.

## Naming boundary (maintainer ruling 2026-09-14, spec.md top)

- The product is **total-recall** (crate, binary, MCP registration key,
  `mcp__total_recall_*` surface). **exomemory** names the design pattern /
  memory-root instance the server manages — different systems; the server's
  identity never borrows the pattern's name.
- Frozen contract names in the exomemory namespace stay as they are:
  `EXOMEMORY_DIR` / `EXO_*` env names, and the dot-dirs under the root
  (`.claims/`, `.audit/`, `.locks/`, `.state/`, `.index/`).

## Verification bar

- Per change: `cargo test` (163 tests as of 2026-10-30) — green before you
  commit.
- Release changes: `cargo build --release` and run
  `target/release/total-recall --root <path> --self-check` (one JSON line, exit 0).
- Surface changes (descriptions, refusals, tool shapes): a naive-model smoke —
  drive the binary over stdio with a naive client and confirm the refusal
  texts route a model that follows them literally to the right exit. Needs a
  model; CI cannot run it.
- CI (`.github/workflows/`) runs the per-change checks on every push to main
  and every pull request: build and test on Linux and macOS, `cargo fmt
  --check`, `cargo clippy --all-targets -- -D warnings`, and the commit-message
  check below.

## Commits

- Every commit message follows Conventional Commits
  (<https://www.conventionalcommits.org/en/v1.0.0/>): `type(scope)!: description`,
  with type one of `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`,
  `refactor`, `revert`, `style`, `test` (`committed.toml`). CI checks every
  non-merge commit a push or pull request adds; merge commits and the history
  before 2026-10-24 are not checked. To check before committing:
  `committed --config committed.toml HEAD` (install with
  `cargo install --locked committed`).
- No attribution trailers: no `Co-Authored-By`, no "Generated with" line.
- `origin` is public — pushing publishes.

## Releases

Releases are manual and maintainer-triggered. Agents never create tags or
releases by hand — `.github/workflows/release.yml` does it:

1. The version bump is its own commit or pull request: `version` in
   `Cargo.toml` and the matching entry in `Cargo.lock` (CI's `--locked`
   enforces the sync), plus — optionally — a `## [X.Y.Z]` section in
   `CHANGELOG.md`, which becomes the release notes. Without that section the
   workflow builds the notes from the commits since the last tag.
2. A maintainer dispatches Actions → Release → Run workflow with that version:
   `publish` unchecked is a dry run that prints the notes into the run
   summary and tags nothing; checked, it tags `vX.Y.Z` at the run's commit and
   creates the GitHub release. The workflow refuses a version that differs
   from `Cargo.toml` or is already tagged.
3. The Homebrew formula (`jgavinray/homebrew-tap`, `Formula/total-recall.rb`)
   is a manual follow-up: update its `url` and `sha256` to the new tag's
   tarball.

Do not run `git tag` or `gh release create` outside that workflow, and do not
bundle the version bump with product changes — the bump commit is what the tag
names, so it should carry exactly the released tree.
