# totalrecall

A single-binary MCP server (JSON-RPC 2.0 over stdio) that turns the Exomemory
Pattern — markdown side-band memory under one configured root, shared by an
orchestrator session per day and its workers — into an enforced API for the
buckets it owns. The pattern in one line: *the transcript is disposable and the
files are the memory* (`~/dev/exomemory/wiki/research/exomemory-pattern-canonical.md`
is the pattern of record). `spec.md` in this repo is the binding contract; this
README is a doorway, not one.

## What this is NOT

- **Not pattern-locked.** The memory root is a configured value, by
  precedence: `--root` flag > `EXOMEMORY_DIR` env > legacy `EXO_DIR` alias
  (the server refuses loudly if both env names are set to different paths —
  `src/config.rs`) > `~/.config/exomemory/config.toml` key `root` > default
  `~/dev/exomemory/` (spec §2). The default is the reference deployment's
  convenience, never part of the contract.
- **Not a wiki or CMS.** `wiki/`, `inbox/`, `topics/`, `signoffs/` are declared
  out of scope (spec §10); no server tool reads or writes them.
- **No generic write, no delete, no network.** There is no `edit_file`,
  `delete_file`, `write_file`, `rename`, arbitrary-path read, shell/exec, or any
  outward call — a forbidden act has no tool, so it cannot be argued with.
  Absence IS the api (spec §3 EXCLUSIONS).

## Build & run

```sh
cargo build --release                              # binary: target/release/totalrecall
target/release/totalrecall --root <path> --self-check
```

`--self-check` resolves the memory root, runs the init-fingerprint startup gate,
prints one JSON line `{root, tz, tz_offset_minutes, session}` and exits 0
(`src/main.rs`). Without the flag the server serves newline-delimited JSON-RPC
2.0 over stdio; logs go to stderr. Dependencies: Rust std + `serde_json` only,
edition 2021. The derived `.index/` is a cache — rebuildable, never truth.

Register in `~/.omp/agent/mcp.json` under `mcpServers` — this shape is taken
from the real file (which also carries an unrelated `kaibo` registration):

```json
"totalrecall": {
  "timeout": 60000,
  "command": "<absolute path to this repo>/target/release/totalrecall",
  "args": [
    "--root",
    "<absolute path to your memory root>"
  ]
}
```

The registered `--root` MUST be the directory from step 1; the reference
deployment's working entry lives in `~/.omp/agent/mcp.json`.

## ADOPT IT — first-day checklist

1. Create a root directory: `mkdir -p ~/dev/my-memory`
2. Seed `signoff.md` there **by hand**. The server REFUSES an unprovisioned
   root: `read_signoff` on a missing `signoff.md` fails loudly, names
   provisioning as the failure class, and never creates the file — seeding it
   is the human/launcher's documented bootstrap (spec §2/§3). A minimal seed
   is a dated header plus the warm-start block; the parser takes the header
   from the `Last updated:` line directly above the heading:

   ```markdown
   # Signoff

   Last updated: 2026-09-14 — root bootstrapped today; this is the whole list.

   ## If you read nothing else
   1. Nothing is in flight. Read this, work, sign off before you stop.
   ```

3. Register the server (snippet above) and start a session on that root.
4. First call, always: `mcp__totalrecall_read_signoff` — it grants the
   handshake; every gated tool refuses until it has succeeded this session.
   Only `read_signoff`, `recall`, `last_tick`, `session_compliance` are ungated.
5. Orchestrator flow: `mcp__totalrecall_claim_orchestrator {date}` to take
   today's single-writer token, then `mcp__totalrecall_write_dayfile {content,
   orchestrator_token}` to record the day (`write_brief` / `write_warm_start`
   are claim-gated too).
6. Worker flow: end every session with `mcp__totalrecall_append_signoff
   {role, workflow, done, …}` — one verbatim line, appended under an exclusive
   lock.
7. Acceptance — the kill-test: kill any session mid-run and lose only the last
   few minutes. If losing a session would hurt, its state was not written down
   yet; fix that, not the session (spec §12).

## Tests

`cargo test` — currently 135 passing across 15 test binaries. Known flake: the
real-time lock/race assertions in `tests/signoff_append_tests.rs` (bounded
retry ~5 s, stale-lock takeover < 4 s) can fail under heavy machine load; they
pass in isolation.

## Layout

| Path | Role |
|---|---|
| `spec.md` | the binding contract (v0.3) |
| `src/` | server implementation (`main.rs`, `rpc.rs`, `gate.rs`, `config.rs`, `tools/`) |
| `tests/` | enforcement gates (`gate_w1..w4`, `rpc_tests`) + per-module tests |
| `docs/workflows.md` | mermaid diagrams of the enforcement plane |
| `signoffs/` | review artifacts produced by the guard/human path, not by the server |

## Licence

GPL-2.0 — see `LICENSE`.
