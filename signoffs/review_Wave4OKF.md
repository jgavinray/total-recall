# kaibo combined-diff review — wave 4 + protocol + audit (job-2, cast vllm-local @ 2026-09-14)

Verbatim verdict for the uncommitted working tree (spec v0.3, rpc registry/envelope/serverInfo, audit insertions ×5 tools, wave-4 modules + tests). Token counts: 804251 in / 24727 out / 19027 reasoning.

# Verdict: **SHIP WITH CHANGES**

The wave-4 modules are substantially conformant: the `recall` schema is byte-for-byte the spec §3 shape, the gate split is exactly §4, and the derived index obeys every invariant the amendment names. The two changes that must land before commit are the audit-write failure semantics (an `isError` that follows a landed write) and the stale-lock takeover race. Everything else is MINOR/NIT.

---

## 1. SPEC CONFORMANCE

### 1a. Tool schemas — PASS

`recall`'s four-field schema with `additionalProperties:false` and the `dayfile|signoff|briefs|all` enum (no `wiki`) is exact:

```rust
// src/tools/recall.rs:55-65
input_schema: json!({
    "type": "object",
    "properties": {
        "query": {...},
        "scope": {"type": "string", "description": "Bucket to search", "enum": ["dayfile", "signoff", "briefs", "all"], "default": "all"},
        "workflow": {...},
        "since": {...}
    },
    "required": ["query"],
    "additionalProperties": false
}),
```

`additionalProperties:false` is enforced by hand (serde_json does not validate schemas), and the unknown-field refusal names the allowed set — which is what makes a path-escape attempt arrive as a refused field:

```rust
// src/tools/recall.rs:118-132
let unknown: Vec<&str> = args.keys().filter(|key| !matches!(key.as_str(), "query"|"scope"|"workflow"|"since"))...
if !unknown.is_empty() {
    return HandlerResult::Err(format!("recall refused: unknown field(s) {unknown:?} — the schema allows only: query, scope, workflow, since"));
}
```

`log_tick` / `last_tick` / `session_compliance` schemas match spec §3 (`src/tools/ticks.rs:62-70`, `:75-82`, `src/tools/compliance.rs:50`).

### 1b. Gate split — PASS

`log_tick` is gated; `last_tick`, `recall`, `session_compliance` are ungated, exactly §4:

```rust
// src/tools/ticks.rs:120
if let Err(refusal) = gate::gate(server, session) { return HandlerResult::Err(refusal); }
// src/tools/ticks.rs:207  (last_tick — no gate call)
// src/tools/recall.rs:103 (recall — no gate call)
// src/tools/compliance.rs:71 (session_compliance — no gate call)
```

All six gated tools call the gate (`signoff_append.rs:138-145` inline, `claims.rs:278`, `claims.rs:675`, `briefs.rs:108`, `warmstart.rs:119`, `ticks.rs:120`).

### 1c. Derived `.index/` invariants — PASS

- **never source-of-truth / every excerpt re-read live / ts = live mtime**: `recall` re-reads each candidate and stamps the live mtime:
```rust
// src/tools/recall.rs:249-276
let text = std::fs::read_to_string(&candidate.file.abs)...;
let live_mtime = std::fs::metadata(&candidate.file.abs).and_then(|m| m.modified())...;
let ts = gate::utc_iso8601(live_mtime);
```
- **rebuilt, never served on corruption**: `read_manifest_json` returns `None` on absent/unparseable/not-object/wrong-version (`src/tools/index.rs:336-346`), and `snapshot_with` builds from disk regardless (`:317-331`). The manifest is never consulted for narrowing — `candidates` reads only the in-process snapshot (`:458-478`).
- **lock + atomic rename**: `persist` takes `.locks/index.lock` and writes tmp+rename (`src/tools/index.rs:595-639`).
- **best-effort post-write**: every `persist` failure logs and returns (`:597-638`); no append path calls into the index.
- **no new tool**: registry has no index tool (`src/rpc.rs:129-150`).
- **std+serde_json only**: `std::os::unix::fs::MetadataExt` for inode (`src/tools/index.rs:62`); `Cargo.toml:8-9` pins only `serde_json`.

### 1d. FINDING (MINOR) — `capabilities` deviates from the spec text

Spec §2 pins `capabilities: {"tools": {}}` (spec.md:34), and the amendment repeats "capabilities stay `{"tools": {}}`". The implementation emits an extra field:

```rust
// src/rpc.rs:332
"capabilities": {"tools": {"listChanged": false}},
```

`gate_w1.rs:126` only asserts `listChanged` is a boolean, so it passes either way. **Fix:** either drop `listChanged` to match the spec literally, or amend spec §2 to `{"tools": {"listChanged": false}}`. Do not leave the contract and the code disagreeing.

---

## 2. SECURITY / CORRECTNESS

### 2a. Path confinement — PASS

`bucket_files` canonicalizes every enumerated entry and refuses anything resolving outside the root; symlinked bucket entries are excluded at enumeration because `file_type()` reports the link, not the target:

```rust
// src/tools/index.rs:178-192
let inside = abs.canonicalize()...;
if !inside.starts_with(&base) {
    return Err(format!("index refused: bucket entry {abs:?} resolves outside the configured memory root ({inside:?}) — path confinement refused"));
}
// src/tools/index.rs:812-818
matches!(entry.file_type(), Ok(ft) if ft.is_file())  // symlink reports the LINK
```

The scope enum is the only bucket selector; there is no path parameter (`recall.rs:154-172`). `tests/index_tests.rs:431-569` exercises traversal, absolute paths, a path-shaped query, a symlinked brief file, and a symlinked `briefs/` directory — all refused or excluded.

### 2b. FINDING (MAJOR) — stale-lock takeover is an unconditional `remove_file` (TOCTOU)

The takeover path checks age, then removes the path without re-verifying identity:

```rust
// src/tools/index.rs:710-715
Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
    if lock_age(&path).is_some_and(|age| age >= STALE_AFTER) {
        let _ = std::fs::remove_file(&path);   // <-- removes WHATEVER is there now
        continue;
    }
```

Race: A and B both read a stale lock; A removes it and creates its own fresh lock; B (still acting on its stale read) removes **A's fresh lock** and creates its own. Both now believe they hold the mutex, and the critical section interleaves. The same pattern is copied verbatim in `signoff_append.rs:413-419`, `claims.rs:876-883`, `warmstart.rs:608-614` — so this is inherited, but the new `.locks/index.lock` and `.locks/audit-<date>.jsonl.lock` inherit it too. Release is identity-checked (`index.rs:764-787`), so a successor's lock is not deleted on release — but the mutual-exclusion break already happened.

**Fix:** re-read the lock immediately before removal and only remove if its `epoch`/`nonce` still match the stale record you measured; or take over by atomic `rename` of the stale lock to a unique name and then `create_new`. A single shared lock helper (rather than four copies) would also let this be fixed once.

### 2c. Lock release on error paths — PASS

Every acquire/release pair releases on all exits: `perform_append` (`signoff_append.rs:515-520`), `replace_dayfile_under_lock` (`claims.rs:759-770`), `write_warm_start` (`warmstart.rs:160-165`), `persist` (`index.rs:608-638`), `append_audit` (`ticks.rs:330-344`). No lock is nested inside another (signoff/day-file lock is released before `audit_write`), so no deadlock.

### 2d. FINDING (MAJOR) — audit-write failure returns `isError` after the main write landed

Every mutating tool calls `audit_write` **after** its main write, and turns an audit failure into `HandlerResult::Err`:

```rust
// src/tools/signoff_append.rs:184-191
if let Err(err) = crate::tools::ticks::audit_write(&root, &server.session_id, "append_signoff") {
    return HandlerResult::Err(format!("append_signoff: the line landed at {} but its audit entry did not — {err}", ...));
}
```

Same shape at `claims.rs:736-744` (write_dayfile), `briefs.rs:158-164`, `warmstart.rs:168-174`, and `claims.rs:357-366`/`:384-392`/`:428-438` (claim). The protocol layer renders `Err` as `{isError:true, content:[…]}` (`rpc.rs:280-283`). So the client observes `isError:true` **and** the signoff line / day file / brief is on disk — the exact "says isError and writes anyway" condition spec §8 declares a FAIL. Spec §2 scopes `isError` to "refusal, validation error, gate denial", none of which an audit I/O failure is.

**Fix:** return `HandlerResult::Ok` carrying an `audit_error` field (the write landed; report the missing tail without claiming failure), or make the audit append best-effort (log to stderr) and keep the success result. If the intended contract really is "fail the call", spec §8 must carve out this post-write case explicitly.

### 2e. Two-process-one-root with the new lock names — PASS

`.locks/index.lock` (`index.rs:81`), `.locks/audit-<date>.jsonl.lock` (`ticks.rs:329`), `.locks/signoff.md.lock` (`signoff_append.rs:47`), `.locks/<date>.md.lock` (`claims.rs:733`) are all distinct names, so no cross-tool collision. `tests/index_tests.rs:315-386` drives the real binary as process B and confirms the reader re-validates and finds B's line.

### 2f. FINDING (MINOR) — narrow-scope `recall` is coupled to all-bucket readability

`recall` enumerates the requested scope (`recall.rs:218`) but then builds the snapshot over **all** buckets:

```rust
// src/tools/recall.rs:230
let snapshot = match index::snapshot_with(root, true) { ... };   // snapshot_with calls bucket_files(root, Scope::All)
```

So a `scope:"signoff"` query refuses if an unrelated bucket (e.g. a `briefs/` symlink or an unreadable brief) cannot be resolved. "Fail loud" is defensible, but it means a scoped search can be denied by a bucket it was never asked to read. **Fix:** build the snapshot over the requested scope, or document the coupling.

---

## 3. TEST HONESTY

### 3a. gate_w4 fixture corrections — meaning survived (one disclosed weakening)

**(i) hardcoded `.audit` date → local-date helper** (`tests/gate_w4.rs:216-222`). The audit file is named by the root's local date (`ticks.rs:322`), so a hardcoded `2026-09-13.jsonl` was a fixture bug. The assertion (`j["check"]`, `j["result"]`, `j["session_id"]`, `ts` ends with `Z`) is unchanged. **Meaning survived.**

**(ii) query `'Parser edge'` → `'Worker signoff (beta)'`** (`tests/gate_w4.rs:137`). The original query does not occur in `signoff.md` (the SEED at `:47-62` has no "Parser edge"), so `scope:"signoff"` would have returned 0 and the `count==1` assertion could never pass — a genuine fixture bug. But the corrected term occurs **only** in `signoff.md`, so the test no longer proves confinement (a scope-ignoring bug would still return count 1). This is a real loss of teeth, and it is **disclosed, not silent**: `tests/index_tests.rs:18-22` states "confinement carries the weight gate_w4 test 3 can no longer carry", and `scope_confinement_is_what_narrows_the_answer` (`:575-617`) pins the contrast (same term in two buckets under `all`, one under `briefs`, none under `signoff`). **Meaning survived via compensation; flag as MINOR, not MAJOR.**

**(iii) two fixtures gain a seeded `signoff.md`** (`tests/gate_w4.rs:210`, `:296`). `read_signoff` legitimately refuses a root with no `signoff.md` (`signoff_read.rs:104-111`), so the handshake premise needs the bucket. No assertion touched. **Meaning survived.**

### 3b. claims_tests adaptation — genuinely pins the invariant

```rust
// tests/claims_tests.rs:349-365
if let Ok(rd) = std::fs::read_dir(dir.join(".locks")) {
    for entry in rd { assert_ne!(name, format!("{d}.md.lock"), "the token gate runs BEFORE any lock is acquired"); }
}
let audit = dir.join(".audit").join(format!("{d}.jsonl"));
let trail = std::fs::read_to_string(&audit).expect("the claim's own audit entry exists");
assert_eq!(trail.lines().count(), 1, "the refused write appended no audit entry — only the claim's own");
```

The original `!.locks.exists()` was wrong because the successful claim's own `audit_write` creates and empties `.locks/` (`ticks.rs:330-344`). The replacement checks the specific day-file lock is absent **and** the audit trail has exactly one line (the claim's). A refused `write_dayfile` returns before the lock and before `audit_write` (`claims.rs:724-728`), so if it appended an audit entry the count would be 2. **The assertion genuinely pins "refused write appends no audit entry."** Meaning survived.

### 3c. No other pinned assertion was weakened

`gate_w2.rs:117-118` still asserts no `.audit` before handshake (a refusal path, so the new `audit_write` insertions do not touch it). `gate_w1.rs:126` still pins `listChanged` as boolean. `rpc_tests.rs:284-299` pins the exact ten-name registry; `:434` pins the long refusal text for the wave-3 tools.

---

## 4. THE DELIVERY GAP — PASS

- **unknown tool → -32602**: `rpc.rs:251` (named) and `:287` (missing name); never -32601.
- **unknown method → -32601**: `rpc.rs:291`.
- **tool failure → result with `isError` + `content`**: `rpc.rs:280-283`.
- **success → single text item wrapping serialized handler JSON, no raw-field leak**: the envelope is built only at the dispatch point:
```rust
// src/rpc.rs:264-267
HandlerResult::Ok(result) => match serde_json::to_string(&result) {
    Ok(text) => Ok(json!({"content": [{"type": "text", "text": text}]})),
```
`rpc_tests.rs:452-487` asserts `content.len()==1`, `type=="text"`, and `v["result"].get("claimed").is_none()`.
- **`serverInfo` name/version**: `rpc.rs:312-313` (`exomem-mcp`, `0.3.0`), emitted at `:333`; pinned by `rpc_tests.rs:391-392`.
- **notification silence**: `rpc.rs:230-236` returns `Ok("")`; `rpc_tests.rs:127-144` and `:330-377` confirm.

The only delivery-gap deviation is the `capabilities` extra field (finding 1d).

---

## FINDINGS

| # | Tag | Finding | file:line | Fix |
|---|---|---|---|---|
| F1 | **MAJOR** | Audit-write failure returns `isError` after the main write landed — violates spec §8's both-halves rule | `signoff_append.rs:184-191`, `claims.rs:736-744`, `briefs.rs:158-164`, `warmstart.rs:168-174`, `claims.rs:357-366` | Return `Ok` with an `audit_error` field, or make the audit append best-effort; if failing is intended, carve the case out in spec §8 |
| F2 | **MAJOR** | Stale-lock takeover is an unconditional `remove_file` after an age check — TOCTOU lets two writers hold the mutex | `index.rs:710-715` (and copies `signoff_append.rs:413-419`, `claims.rs:876-883`, `warmstart.rs:608-614`) | Re-verify the stale record's `epoch`/`nonce` immediately before removal, or take over via atomic rename; consolidate into one shared lock helper |
| F3 | MINOR | `capabilities` emits `{"tools":{"listChanged":false}}` but spec §2 pins `{"tools":{}}` | `rpc.rs:332` vs spec.md:34 | Drop `listChanged`, or amend spec §2 |
| F4 | MINOR | `gate_w4` test 3 lost its confinement teeth (query now occurs only in `signoff.md`) | `gate_w4.rs:137` | Restore a term that exists in ≥2 buckets, or accept the disclosed compensation in `index_tests.rs:575-617` |
| F5 | MINOR | Narrow-scope `recall` refuses if an unrelated bucket is unreadable (snapshot built over `Scope::All`) | `recall.rs:230`, `index.rs:317-318` | Build the snapshot over the requested scope |
| F6 | MINOR | Regex `REP_CAP=256` silently caps `{m}`/`{m,}` with m>256, returning a wrong "no match" instead of failing loud | `index.rs:844-848`, `:1244-1245` | Refuse loudly when `min > REP_CAP` |
| F7 | NIT | `append_signoff` returns the short refusal text; the other five gated tools return the long form | `signoff_append.rs:67` vs `gate.rs:49-50` | Unify on one text (both are spec-sanctioned, but the gate should read uniformly) |
| F8 | NIT | `[\d]` inside a character class is treated as literal `d`, inconsistent with `\d` outside a class | `index.rs:1126-1145` | Handle `\d`/`\w`/`\s` inside classes |
| F9 | NIT | Archived briefs (`briefs/<worker>-…Z.md`) are enumerated as `scope:"briefs"` hits | `index.rs:215-230` | Decide whether archives are in the briefs bucket; document either way |

---

## MUST FIX BEFORE COMMIT

1. **F1** — audit-write failure must not produce `isError` after a landed write (or spec §8 must explicitly exempt it).
2. **F2** — make stale-lock takeover identity-safe (re-check before `remove_file`, or rename-based takeover), and apply it to all four lock copies including the new `index.lock` / `audit-*.jsonl.lock`.
3. **F3** — reconcile `capabilities` with spec §2 (drop `listChanged` or amend the spec).

F4–F9 are acceptable to ship as-is if recorded, but F4 and F5 should at least be noted in the wave-4 signoff so the confinement coverage is not assumed to live in `gate_w4` test 3.

———
kaibo · cast `vllm-local` · explorer `DeepSeek-v4.1-Flash-EXL3` · synth `DeepSeek-v4.1-Flash-EXL3`
tokens · 804251 in · 24727 out · 19027 reasoning
