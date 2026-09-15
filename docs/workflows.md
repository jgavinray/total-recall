# total-recall — workflow diagrams

Companion view to `spec.md` (v0.3): how the workflows actually run, as built in commit 0a1a4b1.
The load-bearing reading: diagrams 2 and 3 encode the two invariants everything else hangs on —
a refusal never touches disk, and the index never answers from cache. Both are pinned by tests,
not by hope.

## 1. Session lifecycle — the handshake gate (spec §4)

```mermaid
sequenceDiagram
    participant S as Session (worker / orchestrator)
    participant R as rpc.rs dispatch
    participant G as gate.rs
    participant D as Disk (memory root)

    S->>R: tools/call append_signoff
    R->>G: gate check (no handshake yet)
    G-->>R: refused + counted (session_compliance sees it)
    R-->>S: isError "handshake incomplete — call read_signoff first — memory protocol: … call it, then retry."
    Note over R,D: zero bytes written — both halves of §8

    S->>R: tools/call read_signoff
    R->>D: read signoff.md (mtime = as_of)
    R->>G: handshake() → handshaken[session]=true, .state/sessions.json
    R-->>S: warm-start block + ranked list + worker signoffs

    S->>R: claim_orchestrator
    R->>D: .claims/orchestrator-<date> O_CREAT|O_EXCL
    R-->>S: token (re-claim after restart returns EXISTING token)

    S->>R: write_dayfile / write_brief / write_warm_start / append_signoff / log_tick
    R->>G: gate ✓ + dual token check (per-session token + claim file)
    R->>D: .locks/ acquire → append or tmp+rename → release → .audit/<date>.jsonl
    R-->>S: content[] envelope (audit failure ⇒ Ok + audit_error, never isError-after-write)

    Note over S,R: always ungated: read_signoff, recall, last_tick, session_compliance
```

## 2. Anatomy of one gated write — every layer a tool call crosses

```mermaid
flowchart TD
    F["JSON-RPC frame (stdio, newline-delimited)"] --> P{"method?"}
    P -->|"unknown method"| E1["-32601"]
    P -->|"tools/call, name not in registry"| E2["-32602 (never -32601)"]
    P -->|tools/call known| V["schema validation<br/>additionalProperties:false enforced by hand"]
    V -->|"unknown field / bad scope"| R1["isError result + pinned refusal text<br/>side effect ABSENT (spec §8 both halves)"]
    V --> G{"handshake gate"}
    G -->|no| R2["refusal + counted at the gate"]
    G -->|yes| LK[".locks/ acquire<br/>O_CREAT|O_EXCL, bounded retry"]
    LK --> ST{"lock exists, age >= STALE_AFTER?"}
    ST -->|yes| ID["identity-safe takeover:<br/>re-read + re-stat, remove only if<br/>pid/nonce/epoch/mtime/inode still match<br/>what was measured stale (review F2)"]
    ST -->|no / taken| LK
    ID --> W["the write: append verbatim (signoff/audit)<br/>or tmp+rename atomic replace (dayfile/warm-start)"]
    W --> REL["release = remove OWN lock only"]
    REL --> AU["audit entry post-write<br/>failure ⇒ Ok + audit_error key (review F1)"]
    AU --> OK["success envelope:<br/>content[{type:text, text:serialized handler JSON}]"]
```

## 3. Recall + the derived index — cache that can never become truth

```mermaid
flowchart LR
    Q["recall {query, scope enum,<br/>workflow, since} — UNGATED"] --> CONF["path confinement:<br/>scope enum IS the selector,<br/>no path param, canonicalize + refuse escapes"]
    CONF --> VAL["validate fingerprint manifest<br/>every call: mtime / size / inode / line_count"]
    VAL -->|mismatch, corrupt, wrong version| RB["silent full rebuild from disk"]
    VAL -->|clean| MEM["in-process memo narrows candidates<br/>(the manifest NEVER narrows)"]
    RB --> MEM
    MEM --> VERIFY["re-read EVERY candidate from live bytes<br/>excerpt = live line, ts = live mtime"]
    VERIFY --> OUT["{results:[{path, ts, excerpt}], count}"]
    MEM -.->|best-effort, never inside an append critical section| REP[".index/manifest.json<br/>pure human-inspectable report<br/>tampered report ⇒ announced + overwritten, never served"]
```

## 4. Ownership planes — who writes what (absence IS the API)

```mermaid
flowchart TB
    subgraph SRV["Server-owned (the ONLY sanctioned write path)"]
        SG["signoff.md appends (verbatim, one line, 16 KiB cap)"]
        WS["warm-start block (whole refresh, tail byte-identical)"]
        DF["day file YYYY-MM-DD.md (single writer: today's claim token)"]
        BR["briefs/&lt;worker&gt;.md (orchestrator-gated)"]
        CL[".claims/ + archive rotation (internal housekeeping)"]
        AU2[".audit/*.jsonl (all five mutating tools, post-handshake)"]
        LX[".locks/ + .state/ + .index/ report (internal, not API)"]
    end
    subgraph HUM["Human / session path — server has NO tool here, by design"]
        WK["wiki/ OKF bundle — index.md + log.md + wiki-lint.sh"]
        IB["inbox/ — formatless capture, 7-day trash rule"]
        SO["signoffs/review_*.md — guard + kaibo gate"]
    end
    HR["omp guard hook (backstop: rule-5 reviewed-or-waived,<br/>no-unsigned-exit)"] -.->|"enforces alongside, never replaced"| SRV
    HUM ---|"recall scope enum excludes wiki/inbox/topics/signoffs — asserted in tests"| SRV
```
