# kaibo consult — OKF-style indexing amendment (job-1, cast vllm-local @ 2026-09-14)

Question: is regular indexing absent from the design; is the implementation being scaved onto a corner; what is the best amendment shape. Verbatim below.

---

# Verdict

**1) The gap is real and confirmed.** Regular indexing is absent from the design by explicit decision, not by omission. The recall tool is specified as a live, line-scoped scan with no index, and the only server housekeeping the spec permits is stale-claim rotation and tmp files.

**2) NO CORNER — with one named failure mode that would create one.** An index over the buckets is compatible with every pinned decision *provided it is a derived, non-authoritative cache and recall verifies every excerpt against the live file*. The corner appears only if the index is made authoritative (recall serves from the index without re-reading disk) or is persisted as a second source of truth. The implementation is not yet "scaved" onto anything: recall does not exist yet, so the amendment lands at the cheap moment.

**3) Ship a derived, non-authoritative index; no new tool, no new dependency, no frontmatter, no MCP resources; wiki/ stays out unless the human re-rules.** Details below.

---

## 1) The gap — confirmed from spec.md

The recall tool is defined as a live scan with no index:

> `spec.md:211` — "Searches the known memory buckets and returns dated excerpts with paths. Every result carries a timestamp/date — no undated facts … Path-confined: searches only the fixed buckets under the configured root (§2); the input carries no path parameter…"

The search mechanism is pinned to substring/regex, explicitly excluding every index technology:

> `spec.md:406` — "recall search: **DECIDED substring/regex search over the markdown buckets** — a bounded, line-scoped, grep-like search (case-insensitive substring + an optional regex mode), **no SQLite, no FTS5, no embeddings**, and explicitly **no third-party search dependency**: a greenfield stdlib-only Rust binary has no SQLite, so the earlier FTS5/keyword decision (taken under the abandoned reference-phase assumption) is superseded."

The earlier FTS5 option was explicitly superseded in the changelog:

> `spec.md:442` — "**Recall search: DECIDED FTS5 / keyword** at that point — SQLite FTS5 over the markdown buckets with stdlib substring/regex as the no-extension fallback; embeddings rejected for now … — *superseded by the Rust ruling round below: the greenfield Rust binary has no SQLite*."

> `spec.md:451` — "**R3 — Recall search:** the FTS5/keyword decision was taken under the reference-phase assumption; a greenfield stdlib-only Rust binary has **no SQLite**. **DECIDED substring/regex search over the markdown buckets** — bounded, line-scoped, case-insensitive substring + optional regex mode (regex mode selected by a literal `re:` prefix on the query); no SQLite, no FTS5, no embeddings, explicitly no third-party search dependency."

The only server housekeeping the design permits is stale-claim rotation and tmp files — and it is explicitly *not* API surface:

> `spec.md:275-276` — "Server housekeeping (stale-claim rotation into `.claims/` archive, tmp files) is internal implementation, not API surface — the no-delete rule constrains the tools, not the server's own disk bookkeeping (§5). **Absence IS the api**: a forbidden act has no tool, so it cannot be called, so it cannot be argued with."

> `spec.md:313` — "**Stale-claim rotation.** A claim whose date is in the past is rotated server-side into `.claims/archive/` on first touch after the day boundary. Rotation is internal housekeeping: the API offers no delete (§3 EXCLUSIONS), and housekeeping cannot be reached through a tool call."

The storage table has no index row — the buckets are `signoff.md`, `YYYY-MM-DD.md`, `briefs/<worker>.md`, and the dot-dirs `.claims/`, `.audit/`, `.locks/`, `.state/` (`spec.md:39-51`). There is no `.index/`.

**Code confirms the gap and the timing.** The registry assembles only the two wave-2 tools:

> `src/rpc.rs:116-121` — `let mut tools = crate::tools::signoff_read::tools(); tools.extend(crate::tools::signoff_append::tools());` … handlers likewise.

The module list has no recall module:

> `src/tools/mod.rs:2-6` — `pub mod briefs; pub mod claims; pub mod signoff_append; pub mod signoff_read; pub mod warmstart;`

And the wave-4 test contract imports modules that do not exist, so its target fails to compile loudly by design:

> `tests/gate_w4.rs:36` — `use exomem_mcp::tools::{compliance, recall, ticks};`

> `tests/gate_w4.rs:8-16` — recall is "UNGATED … case-insensitive substring by default, regex mode when the query begins with the literal `re:` prefix; scopes `dayfile|signoff|briefs|all` … path-confined — only the fixed buckets under the root are ever searched, `wiki/` / `inbox/` / `topics/` / `signoffs/` are never touched."

So the gap is confirmed, and the amendment lands **before** recall is implemented — exactly the cheap moment.

---

## 2) Corner assessment: NO CORNER

Your working hypothesis is correct. Walk each pinned decision:

**(a) Dependency set = Rust std + serde_json only.** An index is trivially std-only: a JSON manifest plus in-memory `HashMap` posting lists. No SQLite, no FTS5, no embeddings, no third-party crate. The current recall (substring/regex) is already std-only, and an index adds nothing to the dependency set.

> `spec.md:33` — "Honest minimal dependency set: **Rust std + serde_json** … and nothing else."
> `spec.md:369` — "keep the dependency set to exactly the §2 minimum (**Rust std + serde_json**, nothing else) so the licence stays simple."

**(b) Absence-IS-the-API: no new write/edit/delete tools.** An index is internal housekeeping, which the spec already carves out of the API surface:

> `spec.md:275-276` — "Server housekeeping … is internal implementation, not API surface … **Absence IS the api**."

No new tool is needed. The index is written by the server, not reachable through `tools/call`.

**(c) Disk-is-truth.** The index is derived from the markdown; the markdown remains the source of truth. Recall must stamp `ts` from the live file's mtime, not the index build time — and that is already the pinned convention:

> `src/tools/signoff_read.rs:7-9` — "the file's mtime as `as_of` (the age of the data — the same mtime convention `recall` uses for its excerpts)"
> `tests/gate_w4.rs:13` — "every result carries `path` … + `ts` (the file's mtime rendered UTC ISO-8601Z) + `excerpt` (the matched line, trimmed)"

**(d) Two server processes on one root must agree.** If the index is in-process, each process has its own — no shared state, no divergence. If persisted, it is a pure cache validated by fingerprint; two concurrent rebuilds derive equivalent content from the same disk, so last-writer-wins is harmless under atomic rename. The markdown, not the index, is what the two processes agree on.

> `spec.md:315` — "Because state lives on disk + under `.locks/` lock files, two server processes on one directory agree on claims and serialize appends; per-process state holds only handshakes."

**(e) Append-only + atomic-rename writes under `.locks/`.** The index is a separate artifact; it never touches `signoff.md`, the day file, or briefs. The byte-preservation contracts stay intact:

> `src/tools/signoff_append.rs:22-25` — "Pre-existing bytes of `signoff.md` are preserved verbatim…"
> `src/tools/claims.rs:40-43` — "the day file `YYYY-MM-DD.md` is replaced byte-exact and atomically (tmp file + rename…)"
> `src/tools/warmstart.rs:32-45` — "every byte outside the rewritten region — all worker-signoff lines — is preserved verbatim … fails LOUDLY … unless the re-read is byte-identical."

**Where a derived index WOULD create a corner — the concrete hazards:**

1. **Stale index served as fact.** If recall serves an excerpt from the index without re-reading the live file, a human direct edit (which the design permits — only server writes are funneled) or a two-process divergence would serve a line that no longer exists. This violates the perishable-facts rule (`spec.md:28`) and disk-is-truth (`spec.md:307-315`). **Mitigation:** the index only narrows candidate files/lines; recall always re-reads the matched file and stamps `ts` from its live mtime. A missed match (a new line the index doesn't know about) is the residual risk — see hazard 3.

2. **Two processes diverging on index state.** If the index is persisted and treated as authoritative, two processes could serve different answers. **Mitigation:** fingerprint validation on every recall makes divergence self-healing; concurrent rebuilds produce equivalent content; atomic rename prevents torn reads.

3. **Index invalidated by human direct edits.** A fingerprint of `(mtime, size, inode)` catches almost every edit, but a same-second, same-size edit can evade detection. This is the one genuine residual correctness hazard. **Mitigation:** recall re-reads the matched file anyway (so excerpts are always fresh); the only exposure is a *missed* match. For a few-hundred-KB corpus, the honest position is that the index's speed benefit is marginal and this window is a real cost — so the index should be optional and always subordinate, and recall should fail loud rather than serve a result it cannot verify.

4. **Index as a new source of truth / new concurrency surface.** If `.index/` is persisted and written without the `.locks/` protocol, it becomes a second mutable shared artifact. **Mitigation:** write it under `.locks/index.lock` with atomic rename, version it, and treat it as pure cache — or keep it in-process and avoid the artifact entirely.

5. **Format/upgrade hazard.** A persisted index is a new on-disk schema; an old or corrupt manifest must be detected and rebuilt, never misread. **Mitigation:** a format-version field; mismatch or parse failure → rebuild (or fail loud).

6. **Path-confinement leak.** An index must not become a path-escape vector. The scope enum *is* the confinement (`spec.md:216`); the index must be built only from the fixed buckets.

None of these is a reversal of a pinned decision — each is a design constraint on the index. So: **no corner, provided the index is never authoritative and recall verifies live.**

---

## 3) Recommended amendment shape

### What I would ship

An **internal, derived, non-authoritative index** over the server's buckets (`signoff.md`, `YYYY-MM-DD.md`, `briefs/*.md`), consisting of a per-file fingerprint manifest `{path, mtime, size, inode, line-count, format-version}` plus parsed-entry lists. It is:

- **std + serde_json only** — no new dependency;
- **never source-of-truth** — the markdown is; recall always re-reads the matched file and stamps `ts` from its live mtime;
- **validated on every recall** — any fingerprint mismatch, corrupt manifest, or version mismatch triggers a silent rebuild (or a loud error if it cannot rebuild);
- **written under `.locks/index.lock` with atomic rename** if persisted, or held purely in-process (my preference for the core, since it adds no shared mutable artifact);
- **not exposed as a tool** — internal housekeeping, like stale-claim rotation.

I would keep the **in-process memoization as the correctness mechanism** and treat an on-disk `.index/` as an *optional, human-inspectable derived report* — never read as truth, regenerable at any time. For this corpus size the index is an optimization, not a correctness requirement; the correctness-critical part is the live verification.

### Section-by-section spec.md edits

**§2 storage table (`spec.md:39-51`)** — add one row:

| Path | Content | Writer | Exists |
|---|---|---|---|
| `.index/` | **DERIVED CACHE** — per-file fingerprint manifest + parsed-entry lists over the server's buckets; rebuildable from disk at any time; never source-of-truth; versioned; written under `.locks/index.lock` with atomic rename | internal housekeeping (not a tool) | created-on-first-write |

And add a sentence after the table: "`.index/` is a derived cache, not a bucket: it may be deleted at any time and recall rebuilds it from the markdown. No tool reads or writes it; it is housekeeping in the sense of §3 EXCLUSIONS."

**§3 tool table (`spec.md:59-72`)** — no new row. Amend the `recall` row's Effect to: "dated excerpts + paths; may use the internal derived index to narrow candidates, but every excerpt is verified against the live file and stamped with its mtime."

**§3 `recall` schema (`spec.md:207-224`)** — no schema change (no new params; the scope enum stays the confinement). Amend the description to add: "The server may maintain an internal derived index over the buckets to narrow candidates; the index is never authoritative — every returned excerpt is re-read from the live file and its `ts` is that file's mtime. A stale, corrupt, or version-mismatched index is rebuilt, never served."

**§3 EXCLUSIONS (`spec.md:275-276`)** — extend the housekeeping sentence: "Server housekeeping (stale-claim rotation into `.claims/` archive, tmp files, the derived `.index/` cache) is internal implementation, not API surface…"

**§10 (`spec.md:371-383`)** — no change unless the human re-rules wiki/ (see shape (b) below).

**§13 (`spec.md:400-407`)** — add a decided bullet: "recall indexing: **DECIDED derived internal index** — a fingerprint manifest + parsed-entry lists over the server's buckets, std + serde_json only, never source-of-truth, validated on every recall and rebuilt on mismatch; no new tool, no new dependency, no frontmatter on server-owned buckets. Revisit only if the corpus outgrows the scan."

**§14 changelog** — add a line recording the decision and that it does not reverse R3 (`spec.md:451`): the search mechanism stays substring/regex; the index is a candidate-narrowing cache, not a search engine.

### New §8 test numbers

Extend the test plan (`spec.md:346-361`) with:

- **13. index rebuild on direct edit** — seed a bucket, run `recall` (builds the index), then edit the file directly (change mtime/size), run `recall` again → the new line is found; the index was invalidated. Asserts disk-is-truth over the cache.
- **14. stale/corrupt index never served as fact** — corrupt or truncate `.index/`, run `recall` → it rebuilds (or fails loud), and every excerpt matches the live file, not the corrupt index.
- **15. two processes, one root, one index** — process A builds the index; process B edits a bucket; process A's `recall` re-validates and finds the new content (no divergence served).
- **16. index never authoritative** — delete `.index/` entirely, run `recall` → results are identical (rebuilt from disk).
- **17. path confinement with the index** — a query/scope crafted to escape the buckets is refused and reads nothing outside the configured root's buckets (extends test 4, `spec.md:353`).
- Amend **test 4** (`spec.md:353`) to state that recall's non-vacuity and path-confinement hold whether or not the index is present.

### What I would explicitly NOT do

- **No new tool** (`index_status`, `rebuild_index`, etc.) — absence IS the api (`spec.md:276`).
- **No new dependency** — no SQLite, FTS5, or embeddings (`spec.md:406`, `spec.md:451`).
- **No OKF frontmatter on `signoff.md`, day files, or briefs** — it would rewrite pinned bytes and break the guard/launcher parsers (`spec.md:131`; `src/tools/signoff_append.rs:5-9`, `src/tools/claims.rs:40-43`, `src/tools/warmstart.rs:32-45`).
- **No MCP `resources` / capabilities change** — capabilities stay `{"tools": {}}` (`spec.md:34`; `src/rpc.rs:273`).
- **No index as source-of-truth** — recall never serves an excerpt it has not verified against the live file.
- **No wiki write path** — wiki/ stays out of scope (`spec.md:47`, `spec.md:379`).
- **No index write inside the append critical sections** — index refresh is post-write and best-effort; it never blocks or alters an append.
- **No index write outside `.locks/` + atomic rename** if persisted.

### Shape-by-shape verdicts

**(a) Internal `.index/*.json`.** Ship it, but subordinate: fingerprint manifest + parsed entries, versioned, validated on every recall, rebuilt on mismatch, written under `.locks/index.lock` with atomic rename. I would prefer the core to be **in-process memoization** (no shared mutable artifact, no divergence surface) and treat the on-disk `.index/` as an optional derived report. Incremental append-tracking is an optimization, not correctness — disk remains truth.

**(b) Read-only recall over wiki/.** Defensible on the spec's stated justification — §10 excludes wiki/ *only* because "a raw server write tool would bypass lint" (`spec.md:379`), and a read-only scope does not bypass lint. But this **reverses a human ruling**, not just a spec line (the context states wiki/ is "outside the server's scope by human ruling"). So: **do not slip it in as a spec edit.** If the human re-rules, add `wiki` to the scope enum (`spec.md:216`), carry frontmatter `type`/`status`/`stale_after` fields on wiki hits when present, keep no path param (the enum is the confinement), and **update `tests/gate_w4.rs:125`**, which currently asserts "wiki/ never searched." My recommendation: keep wiki/ out unless the human explicitly re-decides.

**(c) Index health via log_tick/last_tick.** Attractive, but there is a gate mismatch: `log_tick` is **gated** (`spec.md:280`; `src/gate.rs:3-10`) while `recall` is **ungated**, so an ungated recall cannot record a tick through the tool. Options: (i) the index manifest carries `built_at` and `last_tick` consults it (a new internal tick source — needs a spec sentence); (ii) the server writes the tick as internal audit housekeeping (needs a §2 carve-out); (iii) keep freshness purely internal to recall — fail loud on a stale/unbuildable index, no tick. **I would ship (iii)** as the minimal correct thing, with (i) as an optional admin surface. Do not force it through the gated `log_tick`.

**(d) Out-of-server alternative.** The server owns the buckets, so it should own their index — but only as internal housekeeping, not a tool. A separate periodic tool would be a second writer to the same buckets' derived state and would have to coordinate with the server — strictly worse. The wiki-lint analogy holds only because wiki/ is out of scope; the server's buckets are in scope, so the server owns their index.

**(e) MCP resources.** **No.** Capabilities are pinned to `{"tools": {}}` (`spec.md:34`; `src/rpc.rs:273`), and the design is tools-only. Resources would add a new protocol surface with its own semantics for a single-writer memory server. Freshness belongs in recall's result (`as_of`/`ts`), not in a push channel.

**(f) OKF frontmatter on server-owned buckets.** **No.** The tools carry byte-verbatim contracts: `append_signoff` emits exactly one line (`spec.md:105`; `src/tools/signoff_append.rs:5-9`), `write_dayfile` replaces byte-exact (`src/tools/claims.rs:40-43`), `write_warm_start` preserves the tail byte-identical (`src/tools/warmstart.rs:32-45`). Frontmatter would rewrite pinned bytes and break the guard's schema check (`spec.md:131`). OKF correctly stays to wiki/ only; the server's buckets are append-only logs and single-writer files, not OKF pages. The server already has a perishability signal — the file mtime carried as `ts` — so no `stale_after` frontmatter is needed.

---

**Bottom line:** the gap is confirmed (`spec.md:406`, `spec.md:451`, `spec.md:275-276`, `spec.md:313`); there is **no corner** because a derived index reverses none of the pinned decisions — but it becomes a corner the moment it is made authoritative or persisted as a second source of truth. Ship a derived, non-authoritative, std-only index that recall validates against live files on every call, expose no tool, add no dependency, add no frontmatter, and leave wiki/ out unless the human re-rules.

———
kaibo · cast `vllm-local` · explorer `DeepSeek-v4.1-Flash-EXL3` · synth `DeepSeek-v4.1-Flash-EXL3`
tokens · 323262 in · 14742 out · 9193 reasoning
