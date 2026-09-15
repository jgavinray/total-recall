# Code review — README.md + AGENTS.md (worker: OnboardingDocs)

Verdict from kaibo cast `vllm-local` (DeepSeek-v4.1-Flash-EXL3, DGX Spark), job `job-2`, 2026-09-14. Requested by the human as an independent second opinion; reviewer budget ~20 commands, tokens 339406 in / 16858 out / 14400 reasoning.

**Verdict: SHIP WITH CHANGES.** AGENTS.md clean — every claim (orchestrator-owned registries `src/lib.rs:1-2`/`src/tools/mod.rs:1`, §11-verbatim instructions `src/rpc.rs:324`, naming boundary, commit rule, knowledge paths) checks out. README.md: one functional defect in the ADOPT IT checklist + one internal root contradiction.

## Findings (all resolved pre-commit)

- **[MAJOR, FIXED] Seed example produced an empty header.** The parser takes the header from the nearest non-blank line *before* the heading (`src/tools/signoff_read.rs:200-206`; canonical shape `tests/signoff_read_tests.rs:39-43`; the empty-header outcome for the old shape is pinned at `:130-141`). Fix: `Last updated:` moved above `## If you read nothing else`. Post-fix empirical proof: seeding the README's exact block and calling `read_signoff` live returns `header: "Last updated: 2026-09-14 — root bootstrapped today; this is the whole list."` and `ranked: [{rank:1, …}]`.
- **[MAJOR, FIXED] Checklist root vs snippet root mismatch.** Step 1 created `~/dev/my-memory` while the verbatim snippet served `~/dev/exomemory`. Fix: snippet genericized (`<absolute path to this repo>/target/release/totalrecall`, `<absolute path to your memory root>`) + explicit rule "the registered `--root` MUST be the directory from step 1"; reference entry stays pointed at `~/.omp/agent/mcp.json`.
- **[MINOR, FIXED] Root precedence omitted the legacy `EXO_DIR` alias** + the loud both-env disagreement refusal (`src/config.rs`). Tier inserted.
- **[MINOR, FIXED] README cited `/private/tmp/naive-client-post/REMEASURE.md`** — transient path; removed, behavioral claim kept with spec §2/§3 grounding. grep confirms zero /private/tmp citations remain.
- **Confirmed correct (no finding):** flake claim (`tests/signoff_append_tests.rs:375-378` ≥4500 ms retry, `:414-417` takeover <4 s), `--self-check` shape (`src/main.rs:44-52`), ungated list (`src/gate.rs:9-10`, spec §4:285), 10 tools / std+serde_json / edition 2021 / GPL-2.0 / `.index/` cache / §10 exclusions / spec v0.3.
- Style: none that cause misreading.

———
kaibo · cast `vllm-local` · explorer `DeepSeek-v4.1-Flash-EXL3` · synth `DeepSeek-v4.1-Flash-EXL3`
