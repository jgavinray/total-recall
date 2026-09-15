# Code review — totalrecall round 014f328 (workers: DiscoverabilityFixes, RenameIdentity; remediation verified by RemediateMeasure)

Verdict obtained 2026-09-14 via kaibo cast `vllm-local` (DeepSeek-v4.1-Flash-EXL3, DGX Spark), job handle `job-1`, after four aborted routing attempts (cluster restart, hung sweep, reviewer-side re-delegation, GLM repetition collapse). Reviewer's own numbers: 416812 in / 11502 out / 7920 reasoning tokens.

**Verdict: SHIP** — all substantive checks pass; check 3 could not be diff-verified (git refused in this read-only shell), so it is cleared by static inspection only. [Check 3 subsequently closed by the orchestrator's mechanical `git diff b23f167 014f328 -- src/tools/`: 409 changed lines, zero touching schema keys — description/doc strings and comments only.]

## Findings

**(1) SERVER_INSTRUCTIONS vs spec §11 — PASS.** `src/rpc.rs:321` is line-for-line identical to the spec.md §11 fenced block (`spec.md:398-402`): header `## Memory protocol (MCP: totalrecall)` + the 4 bullets, all four tool names `mcp__totalrecall_*`. Only difference: the constant has no trailing newline (the fenced block does) — immaterial to the line-based pin. Pinned by `tests/rpc_tests.rs:409-420` (each §11 line must ride verbatim).

**(2) Description-vs-handler — PASS.** `claim_orchestrator` "do NOT poll or retry" (`src/tools/claims.rs:121`) matches the held path: a live claim held by another session returns `HandlerResult::Err(claim_conflict_refusal(...))` naming the holder, file left byte-identical (`src/tools/claims.rs:419-424`; text at `:478-480`). No polling exists in the handler. `write_warm_start` heading guard order matches its doc (`src/tools/warmstart.rs:109-118`): gate (`:124`) → arg validation (`:129`) → claim gate (`:140-160`) → heading contract (`:162-172`) → lock (`:177`). Heading guard is after the claim gate and before the lock, exactly as claimed.

**(3) Schema diff — UNVERIFIED (git refused).** `git diff b23f167 014f328` returns `git: external commands are not available in this build of the shell`. Static inspection of every `input_schema` shows no anomaly: all carry `"additionalProperties": false` (`briefs.rs:76`, `claims.rs:127,143`, `compliance.rs:50`, `recall.rs:64`, `signoff_append.rs:91`, `signoff_read.rs:78`, `ticks.rs:69,81`, `warmstart.rs:84`); `required` arrays present where expected; `done` enum `["yes","no"]` (`signoff_append.rs:592`); `recall.scope` enum+default (`recall.rs:59`). What would close the gap: a runnable `git diff` of `src/tools/`.

**(4) signoff_append pre-handshake — PASS.** `src/tools/signoff_append.rs:137` calls `gate::gate(server, session)` exactly once; `gate.rs:60-68` increments `attempted_write_before_handshake` once per refusal and returns `HANDSHAKE_REFUSAL_MESSAGE` verbatim (`gate.rs:48-49`). `rpc.rs:125-128` explicitly does NOT re-wrap gated handlers, so no double-count. Zero refs to the deleted short const: `grep HANDSHAKE_REFUSAL` hits only `gate.rs:48,68` and tests.

**(5) `exomem` residue — PASS.** Every hit in `src/ tests/ Cargo.toml docs/` is `exomemory` (the managed pattern), all legitimate: config default root/env (`config.rs:4,5,35,39,84`), config-file tier (`tests/config_tests.rs:12,115,119,126`), doc-comments (`tests/index_tests.rs:29`, `tests/signoff_append_tests.rs:20`). No `exomem-mcp`/`exomem_mcp` anywhere in scope; `Cargo.toml:2,12,16` name = `totalrecall`. (Note: `signoffs/review_Wave4OKF.md:199` still reads `exomem-mcp`, but `signoffs/` is outside the grep scope — a historical review doc.)

**(6) Deleted/weakened assertions — PASS (with note).** No deleted assertions found. Two `contains`-based checks exist: `tests/signoff_append_tests.rs:111-114` (`contains("handshake incomplete")`) and `tests/gate_w3.rs:129,143,173,247` (holder/claim substrings). The exact contract text is still pinned by equality at `tests/rpc_tests.rs:452-456` (`assert_eq!` full message) and `tests/claims_tests.rs:205` (`assert_eq!(msg, HANDSHAKE_REFUSAL_MESSAGE)`), so no contract weakening. The §11 test uses line-membership (`rpc_tests.rs:416-419`), not byte-equality — intentional per its comment, but it would not catch an extra/duplicated line.

## PASS/FAIL table

| # | Check | Result |
|---|-------|--------|
| 1 | SERVER_INSTRUCTIONS byte-equals spec §11 | **PASS** (line-for-line; no trailing newline) |
| 2 | Descriptions match handlers (claim "do NOT poll"; warm-start guard order) | **PASS** |
| 3 | Schema type/required/enum/default/additionalProperties diff | **UNVERIFIED** — git refused; static inspection clean (orchestrator closed with mechanical git diff: zero schema-key lines in 409 changed lines) |
| 4 | Pre-handshake: one increment/refusal, verbatim message, no short-const refs | **PASS** |
| 5 | `exomem` hits all legit vs residue | **PASS** |
| 6 | No deleted/weakened assertions | **PASS** (exact-text pinned elsewhere) |

**Bottom line:** the rename is clean — identity surface (`serverInfo.name`, package/binary, §11 names, stderr prefix) is `totalrecall`, while the pattern-layer contract (`EXOMEMORY_DIR`/`EXO_*`, dot-dirs, refusal texts, tool names) is untouched. Only residual risk is the un-run schema diff (check 3); no code defect found.

———
kaibo · cast `vllm-local` · explorer `DeepSeek-v4.1-Flash-EXL3` · synth `DeepSeek-v4.1-Flash-EXL3`

> SHA note (2026-09-14): the reviewed commit `014f328` was recommitted as `d4d8a7c` after this review, removing a false `Co-Authored-By:` trailer added by the orchestrating session; the tree is byte-identical (`ce923755…`). The verdict text above is verbatim from kaibo job-1 as delivered against the reviewed content.
