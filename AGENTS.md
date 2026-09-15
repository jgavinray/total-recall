# AGENTS.md — working ON total-recall

Orientation for an agent editing this software (not using it — for that, read
`README.md` and `spec.md`).

## Spec-first discipline

- `spec.md` is the binding contract. Code descriptions are its model-facing
  superset; pinned refusal texts and `inputSchema`s are byte-contracts — NEVER
  paraphrase one in code, and never change one to make a test pass.
- Schemas are FROZEN: an `inputSchema` or pinned refusal-text change requires a
  spec amendment (a human-authorized `spec.md` edit) first, not the other way
  around. The `initialize` `instructions` field is spec §11 verbatim.
- Navigate and edit with the LSP/ast tooling before any `sed`-style rewrite.

## Ownership map

- `src/lib.rs` and `src/tools/mod.rs` are ORCHESTRATOR-OWNED: workers create
  module files only, never edit these registries (their headers say so).
- `spec.md` is orchestrator-owned.
- `tests/gate_w1.rs`..`gate_w4.rs` and `tests/rpc_tests.rs` hold the contract
  pins (refusal texts, envelope shapes, §11 instructions line-by-line).
  Touching a pin needs stated justification in the handoff — a re-pinned
  assertion is a spec event, not a test fix.

## Naming boundary (human ruling 2026-09-14, spec.md top)

- The product is **total-recall** (crate, binary, MCP registration key,
  `mcp__total_recall_*` surface). **exomemory** names the design pattern /
  memory-root instance the server manages — different systems; the server's
  identity never borrows the pattern's name.
- Frozen contract names in the exomemory namespace stay as they are:
  `EXOMEMORY_DIR` / `EXO_*` env names, and the dot-dirs under the root
  (`.claims/`, `.audit/`, `.locks/`, `.state/`, `.index/`).

## Verification bar

- Per change: `cargo test` (135 passing, 15 binaries; the real-time lock/race
  test in `signoff_append_tests.rs` can flake under load, passes in isolation).
- Release changes: `cargo build --release` and run
  `target/release/total-recall --root <path> --self-check` (one JSON line, exit 0).
- Surface changes (descriptions, refusals, tool shapes): a naive-model smoke —
  drive the binary over stdio with a naive client and confirm the refusal
  texts route a model that follows them literally to the right exit.

## Review gate

- A signoff may not claim `done: yes` without the DeepSeek-cluster verdict,
  per the global rule: route the review through
  `~/.omp/agent/skills/deepseek-code-review/SKILL.md` (subagent pinned to
  `spark-vllm/DeepSeek-v4.1-Flash-EXL3`).
- Cluster etiquette: serial calls, ≤4 chat/completions requests per review
  round, one retry on timeout, then fail loudly — never substitute the session
  model's opinion and call it DeepSeek's.

## Commits

- `git commit --no-gpg-sign`. NEVER add a `Co-Authored-By` trailer (hard human
  rule, 2026-09-14). The repo has no remote; the orchestrator commits — workers
  leave the tree dirty unless told otherwise.

## Where knowledge lives

- Durable state goes to `~/dev/exomemory/signoff.md` (verbatim
  `Worker signoff (<role>)` line via `append_signoff`) and the OKF wiki at
  `~/dev/exomemory/wiki/` — never transcripts. Verify by FILE, never by
  session memory.
