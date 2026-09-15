---
title: Total Recall MCP Server — Spec
type: spec
status: v0.3 (2026-09-14)
author: orchestrated assistant for the human
tags: [total-recall, exomemory, mcp, memory, omp, agentic-engineering]
updated: 2026-09-14
---

# Total Recall MCP Server — Spec v0.3

**Naming (human ruling, 2026-09-14):** the project is **total-recall** — package/binary `total-recall` (Rust crate `total_recall`), MCP registration key `total-recall`, tool surface `mcp__total_recall_<tool>` (omp sanitizes non-`[a-z_]` chars to `_`), `serverInfo.name` `total-recall`. "exomemory" names the *design pattern / memory-root instance* the server manages — different systems; the server's identity never borrows the pattern's name. Retained in the exomemory namespace, deliberately: the memory-root contract surface (`EXOMEMORY_DIR`/`EXO_*` env names, the guard), the on-disk bucket layout under the configured root (`.claims/`, `.audit/`, `.locks/`, `.state/`, `.index/`), and every prose reference to the pattern. The §9 prior-art survey (`~/dev/memory/total-recall`) is a separate project that happens to share the name; this one inherits nothing from it.

Single-binary MCP server that turns the side-band memory pattern — markdown files under a **configured memory root** (default `~/dev/exomemory/`, §2 Configuration) — into an enforced API for the buckets it owns. Drafted 2026-09-13; v0.2 reconciles v0.1 with the shipped enforcement plane and the real on-disk format; the build is **authorized** by the human 2026-09-13 (§12).

**Grounding (read, not cited-from-memory):** the canonical statement of the pattern this server enforces is `~/dev/exomemory/wiki/research/exomemory-pattern-canonical.md` (human dictation, 2026-09-14 — "one orchestrator, several workers, and a handful of markdown rituals — the transcript is disposable and the files are the memory"; operational core §7 = this spec's ownership map); the standing rules are `~/dev/exomemory/CLAUDE.md` and `~/.omp/agent/AGENTS.md`; the real entry format is `~/dev/exomemory/signoff.md`; the shipped gates are `~/.omp/agent/extensions/exomemory-signoff-guard.ts` (schema constants :95, `missingSignoffSchema` :194–201, hook registrations :570/:585/:603/:629/:643) and `~/dev/exomemory/scripts/run-workers.sh` (by-FILE token grep gate :54–64); the runbook is `~/dev/exomemory/ops/parallel-workers.md`; the guard's live-proof record is `~/dev/exomemory/wiki/research/omp-signoff-guard-hook-2026-09-05.md`. The prior-art survey is `~/dev/memory/total-recall` (§9). All `~/dev/exomemory/...` citations in this document are **reference-deployment paths** — legitimate evidence about how the reference root behaves, NOT constants any tool may bake in.

## 1. Purpose & thesis

Side-band markdown memory (memory-root pattern: signoff.md warm-start, day files, kickoff briefs, standing rules — at the configured root, default `~/dev/exomemory/`, §2) enforced by a single-writer MCP server **over the buckets it owns**. Within that scope every sanctioned operation is a tool and **no tool exists for a forbidden act** — no `edit_file`, no delete, no generic write; absence IS the api. The server is *not* a universal rule engine: the standing rules it does not own are named in §10 and stay enforced by the shipped omp guard and the launcher's by-FILE gates (§6). Must-not-skip doctrine: memory is the canonical silent-failure operation, so it graduates to a hook at the harness seam — the shipped guard proves that seam works live (…

Mapping — only the rules the server OWNS (each rule marked OWNED here, everything else is §10):

| Standing rule (source) | Mechanism | Status |
|---|---|---|
| Every session starts by reading signoff.md and nothing else (CLAUDE.md:4) | `read_signoff` handshake (§4), mandatory first call | OWNED |
| Single-writer discipline: only orchestrator edits day file (CLAUDE.md:8) | `write_dayfile` requires today's claim token | OWNED |
| Workers append verbatim to signoff.md; never paraphrase a signoff (CLAUDE.md:8) | `append_signoff`, append-only under lock (§2); no edit/delete tool exists | OWNED |
| Warm-start block is triage, not a log; refresh whole, supersede to history (CLAUDE.md:9) | `read_signoff` (ranked view) + `write_warm_start` (the writer, §3) | OWNED |
| Date-stamp facts; treat as perishable (CLAUDE.md:10) | `recall` results carry mandatory timestamps; undated facts unrepresentable | OWNED |
| A check that can't run must fail loudly (CLAUDE.md:11) | `log_tick` / `last_tick` — silence is never good news | OWNED |

## 2. Architecture

- **Single binary — Rust, directly** (human ruling 2026-09-13: "the implementation of this is to be rust NOT python"; edition **2021**) — one Rust binary implementing MCP JSON-RPC 2.0 over stdio (newline-delimited), logs to stderr; no interim reference-implementation phase. Honest minimal dependency set: **Rust std + serde_json** (JSON-RPC serialization — unavoidable), and nothing else. Protocol shapes (fetched from the MCP specification, 2026-09-13):
  - `initialize` → `protocolVersion: 2026-07-28` (the current specification revision at fetch time; the server also accepts earlier revisions it implements), `capabilities: {"tools": {"listChanged": false}}` — tools-only: the server exposes no resources and no prompts, and never emits `notifications/tools/list_changed` (the flag pins the semantic default explicitly), then `tools/list` / `tools/call`.
  - **Unknown tool name** → JSON-RPC error **`-32602`** (Invalid params) — *not* `-32601`, which names an unknown *method*, not an unknown tool.
  - **Tool execution failure** (refusal, validation error, gate denial) → a normal JSON-RPC **result** carrying `isError: true` plus a `content` array — never a JSON-RPC `error` object. §3/§4/§8 examples follow this shape.
- **Storage layout** — every path in the table below is **relative to the configured memory root** (see the Configuration bullet under this table). The `Exists` column is a **dated observation (2026-09-13) of the reference deployment at the default root** — it is not a requirement: a fresh configured root simply starts empty and every path is created-on-first-write.

| Path | Content | Writer | Exists (reference-deployment observation, 2026-09-13) |
|---|---|---|---|
| `signoff.md` | warm-start block + worker signoffs | appends: any handshaken session under an exclusive `.locks/` lock file (§3 `append_signoff`); warm-start block + History: orchestrator via `write_warm_start` | on disk |
| `YYYY-MM-DD.md` | day file: record, decisions, session log | exactly one holder (today's claim token) | on disk (day files present) |
| `briefs/<worker>.md` | kickoff brief per worker | orchestrator-gated | created-on-first-write |
| `topics/<slug>.md` | **OUT OF SCOPE** — no server tool writes it; recall scope excludes it (§10 EXCLUSIONS) | **OUT OF SCOPE** | absent at the reference root (2026-09-13: only `wiki/` carries knowledge pages, under `wiki/topics/`) |
| `signoffs/` | per-role signoff + `review_<role>.md` review artifacts | **OUT OF SCOPE** — owned by the guard/human path (§10); the server never reads or writes it | on disk |
| `inbox/` | cheap capture surface (CLAUDE.md:28–33) | **OUT OF SCOPE** — capture-then-distill, explicitly unformatted and unlinted; no server tool (§10) | on disk |
| `wiki/` | OKF knowledge bundle (CLAUDE.md:18–47) | **OUT OF SCOPE** — wiki-lint-owned bundle; a server write tool here would bypass frontmatter/index/log/lint (§10) | on disk |
| `.claims/orchestrator-<date>` | claim files (`O_CREAT\|O_EXCL`), the claim **source of truth** (§5) | `claim_orchestrator` / server housekeeping | created-on-first-write |
| `.audit/YYYY-MM-DD.jsonl` | append-only audit log (the only other append path) | all mutating tools, post-handshake | created-on-first-write |
| `.locks/` | `O_CREAT\|O_EXCL` lock files — the append mutex, incl. the signoff lock file | internal | created-on-first-write |
| `.index/` | **DERIVED CACHE** — per-file fingerprint manifest + parsed-entry lists over the server's buckets; rebuildable from disk at any time; never source-of-truth; versioned; written under `.locks/index.lock` with atomic rename | internal housekeeping (not a tool) | created-on-first-write |
| `.state/` | init fingerprint, incl. the timezone record used by the startup check (§2 timezone) | internal | created-on-first-write |

`.index/` is a derived cache, not a bucket: it may be deleted at any time and recall rebuilds it from the markdown. No tool reads or writes it; it is housekeeping in the sense of §3 EXCLUSIONS.

- **Configuration** — the memory root is a **configured value, not a baked-in path**. Precedence: 1) `--root <path>` CLI flag; 2) `EXOMEMORY_DIR` env (the guard's own variable — `exoDir()`, exomemory-signoff-guard.ts:135–138; the legacy alias `EXO_DIR` is accepted, and the server refuses loudly at startup if both env names are set to different paths); 3) config file `~/.config/exomemory/config.toml`, key `root`; 4) default `~/dev/exomemory/`. The default is a **convenience for the reference deployment, NOT part of the contract** — any directory that satisfies the layout of this section is a valid root, and nothing in this spec may assume one location.

- **Concurrency model** — every append to `signoff.md` (and to `.audit/*.jsonl`) takes an **exclusive `O_CREAT|O_EXCL` lock file under `.locks/`** for its duration: acquire by racing to create the lock file with **bounded retry**; a stale lock (its holder process is gone) is **detected by age and taken over server-side**; release = the server removes its own lock file at the end of the append. (Advisory file locking is unavailable in Rust std; the lock FILE is the mechanism — the earlier advisory-lock assumption belonged to the abandoned reference phase, see the Rust ruling round in §14.) `O_APPEND` stays for write positioning, but **no bare-atomicity claim is made above `PIPE_BUF` (4096 B)** — the entry-size cap is 16384 B (§3 `append_signoff`), well past `PIPE_BUF`, and the **lock file** carries atomicity, so size never affects it. State-file mutation only via claim tokens (on-disk claim files are the source of truth, §5); no read-modify-write on shared files by un-claimed writers.
- **Session identity** — verified fact, not assumption: omp exports **no** session-id environment variable (`omp://environment-variables.md` documents none; only terminal/OS breadcrumbs like `TERM_SESSION_ID` reach child processes), and the MCP tool bridge injects only the intent field `i`, stripped unless the tool's own `inputSchema.properties` declares it (`omp://mcp-server-tool-authoring.md` §4). Real mechanism: identity is **process-local** — one server process per top-level client session; the server generates its session id at handshake and persists it in `.state/`; an explicit override is honored only if the harness/MCP config env ever supplies one — the override env name is `EXO_SESSION_ID` (any harness/MCP config that supplies it moves the session identity). Server keeps per-session handshake state `{session_id, handshaked, claim_token}`. Handshake state is **process-local**: a server restart requires a fresh `read_signoff` (§8 test 6); claims are *not* process-local (§5).
- **Timezone / day boundary** — the day-file name (`YYYY-MM-DD.md`) and the claim date use the **server's local date** (`EXO_TZ` IANA override; default system local). All emitted timestamps are UTC with a trailing `Z`. A single **configured memory root** is therefore single-timezone **by construction**: the server keys its zone + UTC offset record (in `.state/`) to the resolved root at first init and **refuses loudly at startup** (exit with a printed error, no service) if a later start sees a different zone/offset for the same root — a mixed-TZ multi-box shared root is a misconfiguration, not a supported mode.

## 3. Tools (exact set — expand each with schema)

| Tool | Input | Gate | Effect |
|---|---|---|---|
| `read_signoff` | `{}` | grants handshake | warm-start block + ranked list + worker signoffs |
| `append_signoff` | `{role, workflow, done, unpushed?, awaits_human?, still_running?, kaibo_review?}` | handshake | append verbatim one-line entry; server-stamps `workflow`/`ts`/`session` after the required fields |
| `write_dayfile` | `{content, orchestrator_token}` | today's valid token | replaces day file (single-writer) |
| `claim_orchestrator` | `{date}` | handshake | token (or the caller's retained token), or names current holder |
| `write_brief` | `{worker, brief}` | today's claim token | write `briefs/<worker>.md`, archive superseded (§3 collision-free name) |
| `write_warm_start` | `{content, orchestrator_token}` | today's claim token | rewrite ONLY the warm-start block + History; worker-signoff bytes verbatim |
| `recall` | `{query, scope?, workflow?, since?}` | — | dated excerpts + paths; may use the internal derived index to narrow candidates, but every excerpt is verified against the live file and stamped with its mtime |
| `log_tick` | `{check, result}` | handshake | append tick to audit jsonl |
| `last_tick` | `{check}` | — | latest tick; loud error when registered-but-silent |
| `session_compliance` | `{}` | — | audit-derived compliance report (admin/debug) |

### read_signoff
```json
{
  "name": "read_signoff",
  "description": "MANDATORY FIRST CALL of every session. Returns the signoff.md warm-start block, the ranked 'if you read nothing else' list, and worker signoffs. RECORDS the session handshake: no mutating tool is accepted until this call returns successfully. Contract: first action of every session; no work before it returns.",
  "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
}
```
Example result:
```json
{
  "path": "<configured root>/signoff.md",
  "as_of": "2026-09-13T08:14:02Z",
  "warm_start": {
    "header": "2026-09-13",
    "ranked": [
      {"rank": 1, "text": "Upstream docs PR: docstring-coverage warning fixed; the references-post awaits human approval"},
      {"rank": 2, "text": "Parser golden-suite green at HEAD; packaging decision awaits human"}
    ],
    "history": ["(superseded blocks, newest last)"]
  },
  "worker_signoffs": [
    {"worker": "docs-capture", "ts": "2026-09-12T23:41:07Z", "done": "yes", "workflow": "memory-kernel"}
  ]
}
```

### append_signoff
```json
{
  "name": "append_signoff",
  "description": "Appends this session's signoff verbatim to signoff.md as exactly ONE line: `Worker signoff (<role>) | done: <yes|no> | unpushed: <…> | awaits human: <…> | still running: <…>` — the on-disk format the shipped guard and launcher already parse — with server-appended `| workflow: … | ts: <ISO-8601Z> | session: …` fields only AFTER the required ones, and `| kaibo review: …` when the optional field is present. Append-only by construction under an exclusive `.locks/` lock file; entries (serialized form, incl. server stamps) > 16384 bytes refused — the cap clears the measured longest real entry, 4919 bytes at signoff.md:356 (measured 2026-09-13). REFUSES with 'handshake incomplete — call read_signoff first' unless read_signoff succeeded this session. Last action before any stop/compact/handoff.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "role": {"type": "string", "description": "Worker/delegated-session role — emitted into the fixed token `Worker signoff (<role>)` the guard's schema check and the launcher grep match"},
      "workflow": {"type": "string", "description": "Workflow/delegation this session belongs to (e.g. memory-kernel); server-stamped after the required fields"},
      "done": {"type": "string", "enum": ["yes", "no"], "description": "Completion claim — the `done:` field the guard's rule-5 done-claim regex keys on"},
      "unpushed": {"type": "string", "description": "Committed/complete but not pushed; emitted verbatim as the `unpushed:` field (default 'none')"},
      "awaits_human": {"type": "string", "description": "Blocked on the human; emitted as the `awaits human:` field (default 'none')"},
      "still_running": {"type": "string", "description": "Processes still running on the box; emitted as the `still running:` field (default 'no')"},
      "kaibo_review": {"type": "string", "description": "Review handle, e.g. 'job-12 (cast) @ <iso>', 'n/a (no code changes)', or 'waived (<why>)'; emitted as `| kaibo review: …` after the four status fields when present. The server passes it through verbatim and never synthesizes it; the review gate itself is OUT OF SCOPE (§10) and stays guard-enforced."}
    },
    "required": ["role", "workflow", "done"],
    "additionalProperties": false
  }
}
```
Example request:
```json
{"role": "docs-capture", "workflow": "memory-kernel", "done": "yes", "unpushed": "n/a (no repo writes)", "awaits_human": "none", "still_running": "no", "kaibo_review": "n/a (no code changes)"}
```
Serialized entry (appended verbatim, one line):
```markdown
Worker signoff (docs-capture) | done: yes | unpushed: n/a (no repo writes) | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-13T21:04:52Z | session: s-7c1f9d2e
```

**Gate compatibility (explicit).** The emitted line satisfies the guard checks that inspect the shared file: the token + four-field schema check (`exomemory-signoff-guard.ts:95` `STATUS_FIELDS = ["done:", "unpushed:", "awaits human:", "still running:"]`, :194–201) and the `hasTokenAnywhere`/`session_stop` stop gate, which also searches the shared `signoff.md` (:540–546, :643–676). It does **not** satisfy the launcher's by-FILE verify: `run-workers.sh:54–64` greps the worker's **dedicated** `<cwd>/signoffs/signoff_<role>.md`, a file the server does not write (§10) — a server-only append therefore passes the guard's schema/stop gates but NOT the launcher grep, and producing that dedicated artifact remains the guard/human path. The server's `role` field carries the guard's `EXO_WORKER_ROLE` concept (guard :31, launcher :40), so the two planes cannot disagree about the shared file. Note v0.1's `status` enum and its `## signoff <ts> session=… workflow=…` block example are **deleted**: the fixed four fields carry session state (`done: yes|no` is the completion claim), and server-stamped extras ride after them.

### write_dayfile
```json
{
  "name": "write_dayfile",
  "description": "Writes today's day file (YYYY-MM-DD.md, named by the server's LOCAL date, §2) — single-writer. REFUSES without today's valid orchestrator token: the refusal names the disk-truth state (no claim / held by session=<holder> with acquired_at / orphan pending the 30 s takeover) and the exit — call claim_orchestrator once; if THAT refusal names another holder, stop and report the holder to the human; repeat write_dayfile calls can never land while it is held — nothing written. Never call concurrently with another orchestrator.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "content": {"type": "string", "description": "Full new day-file content (server replaces the file; no merge)"},
      "orchestrator_token": {"type": "string", "description": "Token returned by claim_orchestrator for today"}
    },
    "required": ["content", "orchestrator_token"],
    "additionalProperties": false
  }
}
```

### claim_orchestrator
```json
{
  "name": "claim_orchestrator",
  "description": "Claims orchestrator-ship for a date. The on-disk claim file .claims/orchestrator-<date> is the source of truth (§5): the server re-reads it on every call, never a memory map. Fresh claim: O_CREAT|O_EXCL, returns a token. Same session_id as the recorded holder: returns the EXISTING token — re-claim after a server restart succeeds ONLY when the deployment pins the id via EXO_SESSION_ID (§13): session ids are server-generated per process and never client-visible, so an unpinned new process is a NEW session and cannot re-claim. Different session on a live claim: refused — an isError result naming the holder and reason; that refusal's exits are the human's (no release tool exists by design) — report the named holder, do not poll. Gate for write_dayfile / write_brief / write_warm_start.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "date": {"type": "string", "description": "YYYY-MM-DD; defaults to the server's LOCAL date (§2)"}
    },
    "additionalProperties": false
  }
}
```
Race example — second caller (loser), per §2 protocol shapes (refusal = result + `isError`):
```json
{"isError": true, "content": [{"type": "text", "text": "{\"accepted\": false, \"holder\": \"session=orchestrator-morning\", \"acquired_at\": \"2026-09-13T07:59:00Z\", \"reason\": \"claim held by another session\"}"}]}
```

### write_brief
```json
{
  "name": "write_brief",
  "description": "Writes briefs/<worker>.md (kickoff brief), archiving the superseded version to briefs/<worker>-<superseded-date>T<HHMMSS>Z.md — the UTC timestamp OF THE SUPERSEDED CONTENT (its file mtime, rendered in UTC), so two writes for one worker on one day can never produce the same archive name and no archive is ever overwritten. Orchestrator-gated: requires this session holds today's claim token.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "worker": {"type": "string", "description": "Worker name — becomes briefs/<worker>.md (server validates it as a single safe path segment: no '/', no '..', no absolute path)"},
      "brief": {"type": "string", "description": "One-page kickoff brief: state, today's order of work, decisions made, standing gotchas (template: `templates/kickoff-brief.md` at the reference-deployment root)"}
    },
    "required": ["worker", "brief"],
    "additionalProperties": false
  }
}
```

### write_warm_start
```json
{
  "name": "write_warm_start",
  "description": "Orchestrator-gated (requires today's claim token). Rewrites ONLY the 'If you read nothing else' warm-start block of signoff.md (signoff.md:5) and moves the superseded block to the 'History' section (signoff.md:119) — implementing the 'warm-start = triage, not a log' rule (CLAUDE.md:9), which otherwise has NO writer. Every byte outside the rewritten region — all worker-signoff lines, verbatim — is preserved: the server writes a tmp file and renames atomically under the signoff lock, then re-reads and fails LOUDLY (isError, keeps the old file) unless the non-rewritten tail is byte-identical to the original.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "content": {"type": "string", "description": "Full new warm-start block — INCLUDES its own '## If you read nothing else' heading (the block is written verbatim; a content without that heading produces a block read_signoff will rank as empty — the ranking parser keys on the heading; tests/gate_w3.rs pins the contract). The server moves the previous block to History"},
      "orchestrator_token": {"type": "string", "description": "Token returned by claim_orchestrator for today"}
    },
    "required": ["content", "orchestrator_token"],
    "additionalProperties": false
  }
}
```
Example result:
```json
{"result": {"rewritten": "signoff.md", "warm_start_lines": 14, "history_blocks_moved": 1, "tail_bytes_preserved": true}}
```

### recall
```json
{
  "name": "recall",
  "description": "Searches the known memory buckets and returns dated excerpts with paths. Every result carries a timestamp/date — no undated facts (perishable-facts rule: facts are perishable, re-verify anything older than its natural rate of change). Facts come from recall() results or files, never from context memory. Path-confined: searches only the fixed buckets under the configured root (§2); the input carries no path parameter and any query-derived path must canonicalize inside those buckets. The server may maintain an internal derived index over the buckets to narrow candidates; the index is never authoritative — every returned excerpt is re-read from the live file and its `ts` is that file's mtime. A stale, corrupt, or version-mismatched index is rebuilt, never served.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {"type": "string", "description": "Search terms; case-insensitive substring by default — when it begins with the literal prefix `re:` the remainder is treated as a case-insensitive regex (§13 R3)"},
      "scope": {"type": "string", "description": "Bucket to search", "enum": ["dayfile", "signoff", "briefs", "all"], "default": "all"},
      "workflow": {"type": "string", "description": "Filter to entries whose server-stamped `workflow:` field matches (signoff entries and audit-derived records carry it); results without a workflow tag are excluded when the filter is set. This filter is what backs the per-workflow separability claim in §7."},
      "since": {"type": "string", "description": "ISO date filter (YYYY-MM-DD); omit for full range"}
    },
    "required": ["query"],
    "additionalProperties": false
  }
}
```
Example result:
```json
{"results": [
  {"path": "briefs/parser.md", "ts": "2026-09-12T18:02:11Z", "excerpt": "Parser edge BLOCKED: version mismatch in the fixture loader"},
  {"path": "2026-09-13.md", "ts": "2026-09-13T08:14:02Z", "excerpt": "Decision: bring the Rust port up first (reference parity)"}
], "count": 2}
```

### log_tick / last_tick
```json
{
  "name": "log_tick",
  "description": "Records a check result as {ts, session_id, check, result} in .audit/YYYY-MM-DD.jsonl (the only other append path). Requires the handshake like every other append path (§4). Idle ticks are a local stat of shared files; network/MCP calls on demand, not per-heartbeat.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "check": {"type": "string", "description": "Registered check name (stable identifier)"},
      "result": {"type": "string", "description": "Observed status — report experience, not synthesis"}
    },
    "required": ["check", "result"],
    "additionalProperties": false
  }
}
```
```json
{
  "name": "last_tick",
  "description": "Returns the latest tick for a check. Errors LOUDLY when a registered check (has ticks on record) has aged to or past the tick window (server constant, 45 min; the boundary is pinned by test 11): 'silence is never good news: check X last ticked <ts> (<age> ago)'. Unregistered check is an error too.",
  "inputSchema": {
    "type": "object",
    "properties": {"check": {"type": "string", "description": "Registered check name"}},
    "required": ["check"],
    "additionalProperties": false
  }
}
```
Silent check example (tool-side failure shape per §2 — result + `isError`, never a JSON-RPC error object):
```json
{"isError": true, "content": [{"type": "text", "text": "silence is never good news: check model-server-health last ticked 2026-09-13T06:10:00Z (51m ago)"}]}
```

### session_compliance
```json
{
  "name": "session_compliance",
  "description": "Admin/debug. Audit-derived report: per session — handshake time, first-write time, attempted_write_before_handshake (true when a call to a gated tool was ATTEMPTED and refused before the handshake — observable at the gate; the write itself never lands, so no 'wrote_without_handshake' metric can ever exist), write count, refused count. Compliance is a measured number, not a vibe.",
  "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
}
```

### EXCLUSIONS — the absent API (by design, never to be added)
No `edit_file` / `update_file`, no `delete_file` / remove, no generic `write_file` / overwrite, no `rename` / `move`, no arbitrary-path read (only `recall`, path-confined, over the fixed buckets), no shell/exec, no tool touching `wiki/`, `inbox/`, `topics/`, or `signoffs/` (§10). Server housekeeping (stale-claim rotation into `.claims/` archive, tmp files, the derived `.index/` cache) is internal implementation, not API surface — the no-delete rule constrains the tools, not the server's own disk bookkeeping (§5). **Absence IS the api**: a forbidden act has no tool, so it cannot be called, so it cannot be argued with.

## 4. Handshake enforcement (server-side, harness-independent)

**The gate is uniform over every disk-touching call: all five append paths (`append_signoff`, `write_dayfile`, `write_brief`, `write_warm_start`, `log_tick`) and `claim_orchestrator` refuse until `read_signoff` has succeeded this session.** There is no audit-side exemption — `log_tick` appends to `.audit/` and is therefore gated exactly like the rest. Only the read/audit-side tools (`read_signoff` itself, `recall`, `last_tick`, `session_compliance`) run ungated. All gated tools return `isError: true` results (§2 shapes):

```json
{"isError": true, "content": [{"type": "text", "text": "handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it)."}]}
```

Tool DESCRIPTIONS carry the MANDATORY contract text (most reliably-read context). Refusal messages restate the rule at the failure moment, e.g. `"memory protocol: read_signoff is the first action of every session — call it, then retry."`

```text
# ~25 lines of state; the canonical in-scope enforcement layer (§6)
handshaken = {}   # session_id -> bool  (PROCESS-LOCAL: restart => re-handshake;
                  # claims are deliberately NOT stored here — see §5, disk is truth)

GATED = {append_signoff, write_dayfile, write_brief, write_warm_start, log_tick, claim_orchestrator}

def read_signoff(session):
    handshaken[session] = True
    return load_signoff()

def _gate(fn):                      # wraps every tool in GATED — no exemptions
    if not handshaken[session.get()]:
        return isError("handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).")
    return fn()
```

## 5. Claim lifecycle (disk is the source of truth)

v0.1 kept claim tokens in per-process memory maps while the claim file sat on disk — restart lost the token but not the file, so re-claim hit "claim file already exists" and `write_dayfile` deadlocked. v0.2: **the on-disk claim file `.claims/orchestrator-<date>` is the source of truth** (it stores `{holder_session_id, token, acquired_at}`; created `O_CREAT|O_EXCL`):

- **Re-read always.** `claim_orchestrator` and every token check re-read the file; nothing is cached in memory maps.
- **Same-session re-claim returns the token.** Caller `session_id` == recorded holder → the existing token is returned (idempotent; survives server restart).
- **Different caller** → refused (`isError` result naming `holder` + reason, §3 example). No silent takeover.
- **Orchestrator-death recovery.** After a crash, the same session (same `session_id`) re-claims and gets its token back; any *different* session gets `holder` + reason — takeover is a human decision (e.g. the human directs a fresh claim after the stale holder is rotated), never an automatic race win.
- **Stale-claim rotation.** A claim whose date is in the past is rotated server-side into `.claims/archive/` on first touch after the day boundary. Rotation is internal housekeeping: the API offers no delete (§3 EXCLUSIONS), and housekeeping cannot be reached through a tool call.
- **Day rollover.** At the server's local midnight the *next* date's claim is simply a fresh `.claims/orchestrator-<new-date>` file; the previous day's claim keeps gating nothing (tokens are date-scoped: `write_dayfile` validates the token against the token recorded under *today's* claim date) and is rotated per the bullet above.
- **Two processes, one configured root.** Because state lives on disk + under `.locks/` lock files, two server processes on one directory agree on claims and serialize appends; per-process state holds only handshakes (which are process-local by design, §2/§8 test 6).

## 6. Enforcement planes

Human decision, fixed: **the MCP server is the canonical enforcement plane for its scoped buckets** (signoff append + warm-start, day file, briefs, recall, ticks/audit — the tools of §3). Within those buckets the server is the only sanctioned write path: its handshake gate, claim tokens, `.locks/` lock files, and audit log decide what lands.

**The shipped omp extension guard (`~/.omp/agent/extensions/exomemory-signoff-guard.ts`) remains in force as a subordinate backstop** — it is NOT deleted. It keeps enforcing the rules that are OUT OF SCOPE for the server (§10): the rule-5 reviewed-or-waived gate (`missingReviewGate` :229–265, stop-gate wiring :651–664) and no-unsigned-exit (rule 4; `session_stop` :643–682), and it covers non-MCP write channels (direct file edits by cooperative sessions) where the server can't stand. It enforces at the tool seam via hooks that are **shipped and live-proven** — `session_start` (:570), `before_agent_start` (:585), `tool_call` (:603), `tool_result` (:629), `session_stop` (:643); hook surface documented with its live-proof record at `~/dev/exomemory/wiki/research/omp-signoff-guard-hook-2026-09-05.md` (regression harness: `bun scripts/signoff-guard-test.ts`, expect `53 passed, 0 failed`, per `ops/parallel-workers.md:89-90`). (v0.1's "TO VERIFY omp supports hooks" is deleted — the surface exists and runs every session.)

**The two planes agree on the file, by construction:** `append_signoff` emits the guard's exact legacy format (§3) — the literal token `Worker signoff (<role>)` plus `done:` / `unpushed:` / `awaits human:` / `still running:`, with `| kaibo review: …` and the server stamps after — so the server's entries satisfy the guard's schema and stop-gate checks on the shared file just like hand-written ones (the launcher's dedicated-file grep stays the guard/human path, §10). When the planes ever *disagree* about a file, the file and the guard's by-FILE checks win the argument about what exists; the server wins the argument about what may be *written* through its tools.

Layering, weakest → strongest:

| L | Layer | Mechanism | Independence |
|---|---|---|---|
| 1 | Prompt gates | state-machine phrasing in §11; models follow gates, not virtues | harness prompt only — most droppable |
| 2 | Harness guard (backstop) | shipped omp guard hooks — in-process tool-seam blocks; canonical authority on out-of-scope rules (§10) until those migrate | harness-dependent, live-proven |
| 3 | Server handshake + disk claims | §4 gate + §5 disk-truth claims + §2 `.locks/` lock files — **canonical for scoped buckets**, harness-independent | process-level, unbypassable by prompt |
| 4 | Audit tail | `.audit/*.jsonl` + `session_compliance` → compliance is a measured number | forensic, after the fact |

Interoperability note: both planes resolve from **one configured root**. The guard reads `EXOMEMORY_DIR` (`exoDir()`, exomemory-signoff-guard.ts:135–138 — its `~/dev/exomemory/` fallback is the guard's own convenience default, not a location either plane is allowed to assume); the server resolves the same configured value under the §2 Configuration precedence (flag → env → config file → default), with the guard's variable sitting in that chain so exporting one value moves both planes together. `~/dev/exomemory/` is only the default, never the contract. At startup the server **refuses loudly** when the two planes resolve to different directories (e.g. `--root` or the config file set without exporting `EXOMEMORY_DIR`) — structurally closing the BLOCKER-4 misconfiguration (two planes pointed at two directories).

## 7. Collision model

| Conflict | Mechanism | Guarantee |
|---|---|---|
| project ↔ project / workflow ↔ workflow | `{ts, session_id, workflow}` server-stamped tags + the `recall` **workflow filter** (§3 — the filter that backs this claim; entries without the tag are excluded when it is set) | same-day state separable per workflow |
| agent ↔ agent append (signoff) | exclusive `O_CREAT\|O_EXCL` lock file under `.locks/` per append (bounded-retry acquire; stale lock detected by age, taken over server-side; release = remove own lock file) + 16384 B entry cap; no `PIPE_BUF` atomicity relied on | concurrent appends N=10 (or across two processes, §8): all entries intact, no interleave, size-independent |
| state files (claims / locks) | `O_CREAT\|O_EXCL` claim files, token re-read from disk every call (§5) | two-orchestrator race closed: loser is named, not overwritten; restart-safe |
| dayfile | single token-holder (today's claim) | no write contention; impossible to double-write |
| audit | append-only jsonl, one gated append path | tamper-evident history, derives compliance |

## 8. Test plan — expected-REFUSED tests are first-class

Smoke-test via JSON-RPC over stdio against throwaway configured-root fixtures. Every test names its expected outcome; refusals are assertions, not bug reports. Every expected-refusal test asserts BOTH halves: (a) the response is a result carrying `isError: true` + a `content` array with the expected rule text, AND (b) **the side effect is ABSENT** — the target file's bytes are byte-identical before/after (or the file remains absent), so a server that says `isError` and writes anyway FAILS.

1. **append_signoff before read_signoff** → refusal per §4 shape; `signoff.md` bytes unchanged (no appended line).
2. **log_tick before read_signoff** → refused (uniform gate, §4); `.audit/<date>.jsonl` absent or byte-identical.
3. **write_dayfile without token** → refused (`no valid orchestrator_<date> claim — single-writer dayfile`); the day file is not created/modified. Same pair for **write_brief / write_warm_start without token**: `briefs/` and the `signoff.md` warm block unchanged.
4. **recall on a seeded fixture** → result is NON-EMPTY (fixture seeds ≥1 hit per assertion; a vacuous empty result set FAILS); every hit carries `ts` + `path` (no undated facts); the `workflow` filter separates two workflows seeded on the same day; **path confinement** — queries/scope values crafted to escape the fixed buckets (traversal, absolute paths) are refused and read nothing outside the configured root's buckets. Both the non-vacuity and the path-confinement assertions hold **whether or not the derived `.index/` cache is present** (tests 13–17).
5. **concurrent appends N=10 (one process)** → ten racing `append_signoff` calls: all 10 entries intact, each parses to the fixed line format, no interleave.
6. **server restart between handshake and write** → after restart, the gated call refuses until `read_signoff` re-runs (handshake is process-local); once re-handshaked, the previously claimed token still validates (claims are disk truth, §5).
7. **two server processes on one configured root** → concurrent appends through BOTH processes serialize under the shared `.locks/` lock-file protocol (N=2×10 entries all intact, no interleave); a claim created by process A is honored by process B from disk.
8. **oversized entry** → a 15000-byte serialized entry PASSES (well above the measured real maximum, 4919 B at `signoff.md:356`, and above `PIPE_BUF` — the lock, not the flag, carries it); a 20000-byte entry is refused with **zero bytes written** to `signoff.md`.
9. **tick window boundary** → registered check aged 44:59 → `last_tick` returns normally; aged exactly 45:00 (the window is `age ≥ 45 min`, so the boundary case fires at equality) → loud `isError`, `silence is never good news`.
10. **orchestrator-death re-claim** → same `session_id` re-claims after simulated death/restart and receives the SAME token from disk; a different session is refused with `holder` + reason and the claim file is byte-identical afterward.
11. **brief archive collision** → two `write_brief` calls for one worker on one day produce TWO distinct archives under `briefs/<worker>-<superseded-date>T<HHMMSS>Z.md` (second-granularity UTC stamps of the superseded content); no archive overwritten.
12. **session_compliance** → a deliberately non-compliant scripted session (write-first, handshake-late) is flagged: `attempted_write_before_handshake: true` (gate-observable — the refused attempt, not a write that never lands), refused count = expected.
13. **index rebuild on direct edit** → seed a bucket, run `recall` (builds the index), then edit the file directly (change mtime/size), run `recall` again → the new line is found; the index was invalidated. Asserts disk-is-truth over the cache.
14. **stale/corrupt index never served as fact** → corrupt or truncate `.index/`, run `recall` → it rebuilds (or fails loud), and every excerpt matches the live file, not the corrupt index.
15. **two processes, one root, one index** → process A builds the index; process B edits a bucket; process A's `recall` re-validates and finds the new content (no divergence served).
16. **index never authoritative** → delete `.index/` entirely, run `recall` → results are identical (rebuilt from disk).
17. **path confinement with the index** → a query/scope crafted to escape the buckets is refused and reads nothing outside the configured root's buckets (extends test 4).

## 9. Prior art — reuse decision (DECIDED: greenfield)

`~/dev/memory/total-recall` (origin `git@github.com:jgavinray/total-recall.git`, verified in `origin` config) is already a **Rust stdio MCP server** (`rust-mcp-sdk = 0.8`, features `server, stdio`, Cargo.toml:13) with SQLite + vector search (`rusqlite` bundled + sqlite-vec built via `cc`, Cargo.toml:19-20, :72-73) and ONNX embeddings (`ort = 2.0.0-rc.12`, Cargo.toml:23). Its five tools are `build_context`, `read_note`, `recent_notes`, `search_notes`, `write_note` (`src/mcp/server.rs:333-337`), and `write_note` is **immutable-per-date** — `"Note for {date} already exists (immutability enforced). Use append or a different date."` (`src/mcp/server.rs:56`) — i.e. it already implements the single-writer dayfile trick this spec re-specifies. It pins `ProtocolVersion::V2025_11_25` (`src/mcp/server.rs:377`), one revision behind this spec's `2026-07-28` target. It is **GPL-2.0** (`Cargo.toml:7`; LICENSE file is the GPLv2 text): reusing its code would make this server GPL-2.0, and its dependency set (tokio, rusqlite, ort, …) contradicts the stdlib-only intent of a greenfield port.

**Decision: greenfield** — a stdlib-by-design **Rust** binary (edition 2021) per §2/§12, with the honest minimal dependency set of §2 (**Rust std + serde_json**, nothing else), keeping the absent-API minimalism (§3 EXCLUSIONS). Reuse would still require building the **entire enforcement layer** (claim tokens, lock files, audit log, warm-start enforcement) on top of a note-store codebase — total-recall provides only a note-store immutability pattern (`src/mcp/server.rs:55-56`, `src/memory/store.rs:118-122`), SQLite/VSS + ONNX plumbing (`Cargo.toml:19-20`, :72-73), and five note-surface tools — and **GPL-2.0 is the tie-breaker that rules reuse out**: the new server inherits nothing from total-recall.

The total-recall facts above stand as the **evidence for this decision**, not as an open question. **Licence:** the new implementation inherits nothing from total-recall (no GPL-2.0 obligation from reuse), and **the new project itself is GPL-2.0** (human decision 2026-09-13, "New project is GPL-2.0") — ship `LICENSE` (GPL-2.0 full text) in the repo, and keep the dependency set to exactly the §2 minimum (**Rust std + serde_json**, nothing else) so the licence stays simple.

## 10. Out of scope (declared; NOT the server's rules to own)

The standing rules the server does **not** enforce — each stays with its shipped owner (§6) until an explicit future migration:

| Rule / surface | Why out of scope | Who enforces it |
|---|---|---|
| Rule 5 — reviewed-or-waived: code-writing worker's `done: yes` requires `\| kaibo review: job-N (<cast>) @ <iso>` AND `<repo>/signoffs/review_<role>.md` (≥200 B, same handle) (CLAUDE.md:16; guard rules :20-29, `missingReviewGate` :229-265) | review-gate state (code-write observation, artifact inspection) lives at the tool seam, not in the memory buckets | the guard (`session_stop` :643-682); the launcher's grep gate (`run-workers.sh:54-64`, `ops/parallel-workers.md:92-100`) |
| No-unsigned-exit (guard rule 4) | it is a harness stop-gate, not a memory write | the guard `session_stop` |
| OKF wiki + `inbox/` capture (CLAUDE.md:18-47) | the wiki is a linted OKF bundle (frontmatter/index/log + `scripts/wiki-lint.sh`); a raw server write tool would bypass lint; inbox is by design formatless | the human/session path + wiki-lint |
| "Verify by FILE, never transcript" (AGENTS.md:40-43) | a verification *practice* of the orchestrator and launcher, not an API | launcher + runbook (`ops/parallel-workers.md:30-33`) |
| Ask before outward writes (CLAUDE.md:15) | outward writes are the human's accountable call; the server never touches the network | the human in the loop; refusal surface reserved |

`signoffs/` (per-role signoff + review artifacts) is likewise OUT OF SCOPE for server tooling (§2 storage table): the server's emitted line satisfies only the guard checks that inspect the shared file (§3, gate compatibility); the launcher's by-FILE verify (`run-workers.sh:54–64`) greps this dedicated file, which the server does not write — so producing these artifacts remains the guard/human path.

## 11. Prompt layer (the only prompt that should exist — verbatim)

```text
## Memory protocol (MCP: total-recall)
- First action of every session: mcp__total_recall_read_signoff. No work before it returns.
- Last action before any stop/compact/handoff: mcp__total_recall_append_signoff {role, workflow, done, unpushed, awaits_human, still_running, kaibo_review}.
- Facts come from mcp__total_recall_recall results (dated) or files — never from your context memory.
- The server refuses every append path (including log_tick) without the read handshake. If refused, call read_signoff, then retry. Never write memory-root state except through this server's tools.
```
(4 lines; brevity is compliance. Tool surface names follow omp's pinned convention `mcp__<server>_<tool>` — the tool bridge generates `mcp__<sanitized_server_name>_<sanitized_tool_name>` (`omp://mcp-server-tool-authoring.md` §4); the doubled-separator `mcp__server__tool` form seen elsewhere belongs to a different harness (Claude Code), not omp.)

**Delivery channel (decided 2026-09-14, naive-client round):** this text rides the protocol, not a config file — the `initialize` result carries it verbatim as the MCP-standard `instructions` field (MCP 2025-06-18+ init-result field), which omp's client injects into the model-facing system prompt per connected server (`client.ts` init capture → `manager.ts getServerInstructions` → `sdk.ts rebuildSystemPrompt`, 4000-char cap; no mcp.json instructions key exists). Pinned line-by-line in `tests/rpc_tests.rs` (`initialize_result_carries_serverinfo_and_tools_only_capabilities`); the server registers as `total-recall` in `~/.omp/agent/mcp.json`, so the `mcp__total_recall_*` names resolve.

## 12. Roadmap & build trigger

The Rust binary (edition 2021; greenfield, §9; deps: Rust std + serde_json per §2) → smoke-test JSON-RPC over stdio exactly as §8's fixture harness → wire into the omp MCP server config. **BUILD AUTHORIZED (human ruling, 2026-09-13):** the trigger is overridden — implementation starts now, **in Rust directly** (human ruling 2026-09-13: "the implementation of this is to be rust NOT python" — no interim reference phase), in `~/dev/totalrecall` (git-initialized there; the spec stays the contract). Engineering is pulled by workload as it was, but the human has ordered the build. Acceptance: kill any session mid-run, lose only minutes (the pattern's own test).

## 13. Open questions

- (decided 2026-09-13, cross-family review round) session_id propagation: there is no harness env var to pin — omp exports none (`omp://environment-variables.md`) and the tool bridge injects only the intent field `i`, stripped unless declared (`omp://mcp-server-tool-authoring.md` §4). Identity is process-local: server-generated at handshake, persisted in `.state/`; the explicit override env name is `EXO_SESSION_ID` — honored only when a harness/MCP config actually supplies it (§2, §13).
- (decided 2026-09-13, cross-family review round) memory-root env unification: `EXOMEMORY_DIR` is canonical (the guard's name, :135–138), `EXO_DIR` is a legacy alias, and the server refuses loudly when both are set to different paths (§2/§6).
- (answered 2026-09-13, human ruling — configurable root) there is no canonical box path: each deployment configures its own root (§2 Configuration), and a root shared across boxes is valid provided every writer points at the same configured value and TZ (§2's single-TZ startup rule already refuses the wrong answer). The remaining choice is only which box *runs* the server — that is deployment, not spec.
- (decided 2026-09-13, human ruling — fourth round) reuse: **DECIDED greenfield** (§9); GPL-2.0 rules out reuse — the new server inherits nothing from total-recall.
- (decided 2026-09-13, fifth-round Rust ruling) recall search: **DECIDED substring/regex search over the markdown buckets** — a bounded, line-scoped, grep-like search (case-insensitive substring + an optional regex mode), **no SQLite, no FTS5, no embeddings**, and explicitly **no third-party search dependency**: a greenfield stdlib-only Rust binary has no SQLite, so the earlier FTS5/keyword decision (taken under the abandoned reference-phase assumption) is superseded. Revisit only if queries become conceptual and the corpus outgrows substring/regex — at which point adding rusqlite + FTS5 is a deliberate dependency decision, not the default.
- (decided 2026-09-14, OKF derived-index round) recall indexing: **DECIDED derived internal index** — a fingerprint manifest + parsed-entry lists over the server's buckets, std + serde_json only, never source-of-truth, validated on every recall and rebuilt on mismatch; no new tool, no new dependency, no frontmatter on server-owned buckets. Revisit only if the corpus outgrows the scan.
- Whether workers' dedicated `signoffs/signoff_<role>.md` files eventually migrate behind a server tool, or stay permanently on the guard/human path.

## 14. Changelog — v0.1 → v0.2

Reconciled 2026-09-13 against the mandated DeepSeek review verdict (BLOCK) with the verification-layer corrections applied; human decisions 1–4 (reconcile-don't-build; server canonical / guard backstop; legacy signoff format; rule 5 + wiki out of scope) fixed.
- `append_signoff` now emits the on-disk format line `Worker signoff (<role>) | done: | unpushed: | awaits human: | still running:` (+ optional `kaibo review:`, server-stamped `workflow`/`ts`/`session` after); `role` + `kaibo_review` added to the schema; v0.1's `## signoff … session=… workflow=…` block example and `status` enum deleted.
- New §6 "Enforcement planes": server canonical, shipped guard as live-proven subordinate backstop; "TO VERIFY omp supports hooks" removed.
- Entry cap 4096 → 16384 B with an exclusive append lock (no bare `O_APPEND`/`PIPE_BUF` atomicity claim) — the lock was first specified under the reference-phase assumption and is re-specified as `.locks/` `O_CREAT|O_EXCL` lock files by the Rust ruling round below; measured real maximum 4919 B cited (`signoff.md:356`).
- Claim lifecycle moved to disk-truth (§5): same-session re-claim, stale rotation, death recovery, day rollover, two-process safety.
- New orchestrator-gated `write_warm_start` with schema + example; worker-signoff bytes preserved verbatim via tmp+rename under the lock.
- Storage table: `signoffs/` / `inbox/` / `wiki/` / `topics/` added as OUT OF SCOPE; `briefs/` and dot-dirs marked created-on-first-write.
- `recall` gains the `workflow` filter, backing the §7 separability claim (§3/§7 now agree).
- Handshake gate made uniform over all five append paths including `log_tick` plus `claim_orchestrator`; §4 prose + sketch rewritten.
- MCP protocol shapes corrected: `protocolVersion: 2026-07-28` (fetched), unknown tool → `-32602`, tool failure → result with `isError: true` + `content` array; refusal examples fixed accordingly.
- Test plan: absence-of-side-effect assertions on every refusal test; `wrote_without_handshake` replaced by gate-observable `attempted_write_before_handshake`; recall test non-vacuous on a seeded fixture; new cases for restart, two-process, 15000/20000 B entries, 45-min boundary, death re-claim, archive collision, path confinement.
- Timezone/day boundary defined (§2): local date for dayfile+claim (`EXO_TZ`), UTC `Z` timestamps, loud startup refusal on a mixed-TZ configured root.
- Brief archive name collision-proofed: `briefs/<worker>-<superseded-date>T<HHMMSS>Z.md`.
- §10 "Out of scope" added; the "tools map 1:1 to rules" thesis deleted; §1 table limited to OWNED rules.
- Grounding replaced with live sources (CLAUDE.md, AGENTS.md, signoff.md, the guard, run-workers.sh, ops runbook); the Crush-era page link, `idle-router-mcp`, `work/omp-workflow-kit`, and `test_smoke.sh` references deleted.
- Public-text hygiene: internal codenames/hostnames scrubbed from all examples; tool-name line fixed to the observed `mcp__<server>_<tool>` convention (§11).
- §9 (Prior art) added: `~/dev/memory/total-recall` surveyed (Rust stdio MCP server, rust-mcp-sdk 0.8, sqlite-vec + ONNX, GPL-2.0, immutable-per-date `write_note`); reuse-vs-greenfield left OPEN at that point — since decided, see §9 (DECIDED: greenfield) and the fourth-round block below.

GLM-5.3-Flash-EXL3 cross-family review round (2026-09-13) — five fixes applied to the v0.2 reconciliation:
- F1: gate-compatibility claim corrected — the shared-file line satisfies the guard's schema + stop-gate checks but NOT the launcher's by-FILE grep, which reads the dedicated `<cwd>/signoffs/signoff_<role>.md` the server never writes (§3 gate compatibility, §6, §10).
- F2: memory-root env unified on the guard's canonical `EXOMEMORY_DIR` (guard :135–138); `EXO_DIR` demoted to alias; divergence refused at startup (§2, §6, §13).
- F3: session identity de-assumed — omp exports no session-id env var (`omp://environment-variables.md`); the tool bridge injects only the intent field `i`, stripped unless declared (`omp://mcp-server-tool-authoring.md` §4); identity is process-local, generated at handshake, persisted in `.state/` (§2, §13).
- F4: tool-naming hedge deleted; omp's `mcp__<server>_<tool>` convention pinned (doubled-separator form attributed to Claude Code, not omp) (§11).
- F5: prior-art reuse option re-scoped honestly — total-recall provides note-store plumbing (`src/mcp/server.rs:55-56`, `store.rs:118-122`, `Cargo.toml:19-20`/:72-73), not the enforcement layer; greenfield is the smaller change set; GPL-2.0 is the tie-breaker if reuse is chosen; the choice was OPEN at that point — since decided (fourth-round human ruling below).

Review provenance: DeepSeek-V4.1-Flash (kaibo `job-1`, cast `vllm-local`) = **BLOCK** on v0.1; GLM-5.3-Flash-EXL3 (omp `reviewer` role, `@plan`) = **SHIP-WITH-CHANGES** on the v0.2 reconciliation — 5 fixes applied, zero independent findings beyond DeepSeek's.

Human ruling, third round (2026-09-13): **storage root made configurable** (flag → env → config file → default); the default is a convenience, not the contract (§2 Configuration, §6 interop note, §13).

Human ruling, fourth round (2026-09-13) — two decisions recorded:
- **Reuse vs greenfield: DECIDED greenfield** (stdlib-only Rust binary — the interim reference-phase phrasing of this line superseded by the Rust ruling round below) — reuse would still require building the entire enforcement layer (claim tokens, lock files, audit, warm-start) on top of a note-store codebase, and **GPL-2.0 is the tie-breaker that rules reuse out**; the new server inherits nothing from total-recall (§9).
- **Recall search: DECIDED FTS5 / keyword** at that point — SQLite FTS5 over the markdown buckets with stdlib substring/regex as the no-extension fallback; embeddings rejected for now (stdlib-only greenfield vs the ONNX model + vector index + rebuild pipeline that is total-recall's `ort` + `sqlite-vec` set; the corpus is a few hundred KB of markdown where keyword matching suffices); revisit only if queries become conceptual and the corpus outgrows keyword search (§13) — *superseded by the Rust ruling round below: the greenfield Rust binary has no SQLite*.
- The new project's own licence had been left undecided at that point — since **pinned GPL-2.0** by the implementation-authorization round below (§9 Licence line).

Implementation authorization round (2026-09-13) — two human rulings:
- **S1 — Licence pinned:** "Licence: the new implementation inherits nothing from total-recall (no GPL-2.0 obligation from reuse), and **the new project itself is GPL-2.0** (human decision 2026-09-13, 'New project is GPL-2.0') — ship `LICENSE` (GPL-2.0 full text) in the repo, and keep every dependency zero third-party (stdlib-only per §2) so the licence stays simple." No LICENSE file is added by this spec round; the repo will carry it (§9).
- **S2 — Build authorized:** "**BUILD AUTHORIZED (human ruling, 2026-09-13):** the trigger is overridden — implementation starts now, in `~/dev/totalrecall` (git-initialized there; the spec stays the contract). Engineering is pulled by workload as it was, but the human has ordered the build." (The quote originally named an interim reference-implementation sequence — deleted verbatim by the Rust ruling round below.) The v0.2 build-moratorium clause and the spec-not-authorization framing are retired; the acceptance criterion — kill any session mid-run, lose only minutes — stands (§12).

Rust ruling round (2026-09-13) — three human rulings:
- **R1 — Language:** "the implementation of this is to be rust NOT python" (human ruling, 2026-09-13) — **Rust directly**, edition **2021**, no interim reference-implementation phase; honest minimal dependency set: **Rust std + serde_json** (JSON-RPC serialization — unavoidable) and nothing else; every interim-reference phrasing deleted from §2, §9, §12 and the changelog quotes.
- **R2 — Concurrency mechanism:** Rust std exposes no BSD-style advisory file lock (the earlier mechanism was inherited from the abandoned reference-phase assumption) — appends serialize on **exclusive `O_CREAT|O_EXCL` lock files under `.locks/`**: acquire with **bounded retry**; a stale lock is **detected by age and taken over server-side**; release = the server **removes its own lock file** at the end of the append. The no-bare-`PIPE_BUF`-atomicity statement above 4096 B stands; the **lock FILE** carries atomicity, size never affects it (§2 concurrency model, §7 collision row).
- **R3 — Recall search:** the FTS5/keyword decision was taken under the reference-phase assumption; a greenfield stdlib-only Rust binary has **no SQLite**. **DECIDED substring/regex search over the markdown buckets** — bounded, line-scoped, case-insensitive substring + optional regex mode (regex mode selected by a literal `re:` prefix on the query); no SQLite, no FTS5, no embeddings, explicitly no third-party search dependency. Revisit only if queries become conceptual and the corpus outgrows substring/regex — rusqlite + FTS5 would then be a deliberate dependency decision, not the default (§13).

OKF derived-index amendment round (2026-09-14) — human-authorized amendment, drafted from the kaibo verdict (`job-1`, cast `vllm-local`); the gap it closes (no index over the buckets) was confirmed against §3 `recall`, §13 R3 and the §3 EXCLUSIONS housekeeping carve-out:
- **Recall indexing: DECIDED derived internal index** (§13) — a per-file fingerprint manifest `{path, mtime, size, inode, line-count, format-version}` + parsed-entry lists over the server's owned buckets; **Rust std + serde_json only**, never source-of-truth, validated on every recall and silently rebuilt on mismatch/corruption (a rebuild that cannot run fails LOUD, per the loud-check rule); the on-disk `.index/` is an optional human-inspectable derived report written under `.locks/index.lock` with tmp + atomic rename; the correctness-critical core is in-process memoization; index refresh is post-write best-effort — never inside an append critical section, never altering or blocking an append.
- The amendment explicitly does **NOT reverse R3 — Recall search** (Rust ruling round above): the search mechanism stays substring/regex over the markdown buckets — the index is a **candidate-narrowing cache, not a search engine**; no SQLite, no FTS5, no embeddings, no new tool (absence IS the api), no MCP resources (capabilities stay tools-only per §2 — reconciled to `{"tools": {"listChanged": false}}` by the F3 line below), no frontmatter on server-owned buckets; every returned excerpt is re-read from the live file and its `ts` is that file's live mtime.
- Surfaces amended: §2 storage table (`.index/` row + the after-table derived-cache sentence), §3 `recall` Effect + description (description text only — `inputSchema`, params and the scope enum unchanged), §3 EXCLUSIONS (housekeeping sentence extended with the derived `.index/` cache), §8 (tests 13–17 added; test 4 extended: non-vacuity and path confinement hold with or without the index). The `wiki/` / `inbox/` / `topics/` / `signoffs/` exclusions stand unchanged (§10); wiki/ entered no recall scope.

- **Capabilities reconciled (finding F3, combined-diff review `job-2`, cast `vllm-local` @ 2026-09-14):** §2's pinned `initialize` capabilities now read `{"tools": {"listChanged": false}}` — the value the implementation actually ships (`src/rpc.rs:332`, `listChanged` pinned as a boolean by `tests/gate_w1.rs:126`); the server never emits `notifications/tools/list_changed`, so the semantic default was already `false` and the amendment is editorial: the tools-only / no-resources ruling above stands unchanged, only its quoted form was reconciled with the code (the code was NOT touched for this line).

Naive-client discoverability round (2026-09-14) — six defects from an empirical naive-model study against the release binary (study + transcripts: /private/tmp/naive-client/STUDY.md), fixes as shipped:
- **§3 `claim_orchestrator` description corrected:** re-claim after a server restart is now stated TRUE ONLY under a pinned `EXO_SESSION_ID` — session ids are server-generated per process and never client-visible, so the old verbatim sentence ("re-claim after server restart succeeds") was an unreachable promise for unpinned deployments; the held-claim refusal's exits are stated as the human's (report the named holder, do not poll — no release tool exists by design).
- **§3 `write_warm_start` content field teaches the heading contract** (content must INCLUDE its `## If you read nothing else` heading or the block ranks as empty — previously pinned only in tests/gate_w3.rs, invisible to a model reading the schema).
- **Refusal texts unified (DEFECT-5):** `append_signoff` now emits the shared `gate::HANDSHAKE_REFUSAL_MESSAGE` (was a private short variant) — §4's "the gate is uniform" now holds in code; the two rpc_tests equality pins updated to the long form verbatim.
- **Cold-start livelock (DEFECT-1) mitigated, semantics unchanged:** `read_signoff`'s missing-file error gained the actionable tail (root not provisioned — NOT a call-order problem; retrying cannot fix it; provision or report to the human). Handshake still requires an existing signoff.md; provisioning remains the launcher/human path (matches the prior "two fixtures gain a seeded signoff.md" correction — the precondition is now stated, not discovered).
- **Anti-bypass + exit-path teaching (DEFECT-2/3/6):** write paths carry the "Server-mediated writes ONLY" invariant; `write_dayfile` teaches the claim-first ordering and the stop-and-report exit for a held claim (pinned refusal texts untouched).
- **Description channel:** model-facing descriptions hardened to the kaibo house style (payoff-first, sibling cross-routing, anti-modeling, CAPS invariants) — §3's schemas remain the exact contract; descriptions are its superset.

Product naming round (2026-09-14) — human ruling: **the project is Total Recall, not exomemory** ("Exomemory is the design pattern that we want to mimic in total recall — different systems"). The original title "Exo Memory MCP Server" named the server after what it *serves*, which was always a misnomer once §2 made the root configurable — the binary is pattern-agnostic. Cutover: package/binary `exomem-mcp` → `totalrecall`, `serverInfo.name` → `totalrecall`, MCP registration key → `totalrecall`, §11 tool references → `mcp__totalrecall_*`, §11's final sentence now says "memory-root state … this server's tools" (the pattern's name no longer appears in the server's own identity surface); stderr log prefix `[exomem-mcp]` → `[totalrecall]`; rpc_tests pins updated to match. The contract layer is byte-frozen: `EXOMEMORY_DIR`/`EXO_*`, `.claims`/`.audit`/`.locks`/`.state`/`.index`, tool names, refusal texts — all unchanged, because they belong to the exomemory system the server manages, not to the server.

FOUND-residuals round (2026-09-14) — six wording-level residuals from the post-fix re-measurement (/private/tmp/naive-client-post/REMEASURE.md), fixed in code and re-smoked live (suite 135/0 verified by orchestrator re-run — the worker's reported "145" miscounted its own per-binary list; frames under /tmp/found-nit-smoke/): FOUND-1 the shared gate refusal gained a closing clause (if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem — provision or report; retrying cannot fix it) — pinned prefix byte-intact, §4 example + pseudocode synced verbatim; FOUND-2 the `write_dayfile` token refusal now names the disk-truth state (no claim / held by session=<holder> with acquired_at / orphan pending takeover) and the stop-and-report exit, decided before any lock, zero bytes, three exact claims_tests pins updated; FOUND-3 the claim-conflict refusal states the REAL age-out (the 30 s orphan takeover only — a valid held claim never ages out while its date is current) and that no release tool exists by design; FOUND-4 `SERVER_INSTRUCTIONS` now byte-equal to §11's fenced block INCLUDING the trailing newline (585 B = 585 B on the wire); FOUND-5 `session_compliance` documents the refused_count scope (pre-handshake gate refusals only; post-handshake claim refusals are tool results, never counted); FOUND-6 `warm_start_lines` documented as counting the heading line.

Tick cadence ruling (2026-09-14, delegated to the orchestrator by the human): the pattern's "~30 min" is the EMISSION cadence (a watcher calls `log_tick` at least that often); the server's silence window stays **45 min** — the alarm threshold deliberately sits at 1.5× the rhythm so one skipped beat never cries wolf while genuine death is caught inside 45 minutes (standard watchdog practice: period + grace, one knob each). The two numbers are not in conflict; they are the snooze and the alarm. Test 11's pinned boundary stands.

Naming correction round (2026-09-14) — human ruling: the project spells **total-recall** (hyphenated); the un-hyphenated form of the naming round is retired. Cutover across the identity surface: package/binary `total-recall` (crate `total_recall`), `serverInfo.name`, mcp.json key `total-recall`, §11 references `mcp__total_recall_*` (omp sanitization: lowercase, non-`[a-z_]`→`_`, collapses), stderr prefix `[total-recall]`, README/AGENTS.md/wiki, GitHub repo. Pattern-layer contract names untouched (`EXOMEMORY_DIR`/`EXO_*`, dot-dirs) — they belong to the exomemory system, not the product. §11 byte-equality re-pinned in rpc_tests (585-class byte-compare re-run live).
