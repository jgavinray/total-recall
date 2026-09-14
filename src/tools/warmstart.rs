//! The `write_warm_start` tool (spec §3) — the warm-start block's
//! writer.
//!
//! Pinned semantics (wave-3 contract lines 50-57; spec
//! §2/§3/§4/§5/§8):
//! - Orchestrator-gated. The uniform handshake gate (spec §4) runs
//!   first — a pre-handshake refusal is counted and touches no disk
//!   at all. Then the claim gate (spec §2/§5): the caller-supplied
//!   `orchestrator_token` must equal BOTH the `token` the on-disk
//!   claim record `.claims/orchestrator-<local-date>` carries AND
//!   the caller's per-session stored token (`Server::claim_tokens`,
//!   set by that session's own `claim_orchestrator`) — the claim
//!   file is the disk truth, the per-session map is process-local
//!   and is re-stored by re-claiming after a restart; a claim file
//!   planted on disk alone is NOT a claim. The gate is decided
//!   BEFORE any lock is acquired, so a refusal creates no lock file
//!   and rewrites no byte.
//! - Region surgery: the rewritten region is exactly the wave-2
//!   reader's warm-start region — the `## If you read nothing else`
//!   heading plus, when the nearest preceding NON-BLANK line is a
//!   `Last updated:` line, that header line — running to the next
//!   `## ` heading (or EOF). Missing heading / missing file →
//!   refusal, nothing written. The caller's `content` becomes the
//!   new region (normalized to end with a newline); the superseded
//!   region is moved into the `## History` section — rendered as one
//!   `- ` entry per non-blank line (the entry shape the reader's
//!   `parse_history` already parses: it strips a leading `- `; a
//!   verbatim `## ` heading line moved into the section would
//!   terminate the reader's section scan and orphan the rest of the
//!   block), appended at the section's end (superseded blocks,
//!   newest last) and created at EOF when the file has no section.
//! - Preservation: every byte outside the rewritten region — all
//!   worker-signoff lines — is preserved verbatim: the new file is
//!   composed of the original's own pieces (the head, the middle up
//!   to History, the History section up to the insertion point, the
//!   remainder) plus the rewritten region and the rendered block.
//!   The original is read UNDER the signoff lock (the wave-2
//!   `.locks/` protocol, shared with `append_signoff` — one lock,
//!   one naming scheme, so a rewrite and an append never interleave);
//!   the write is a tmp file + atomic rename under that lock; after
//!   the rename the file is RE-READ and the tool fails LOUDLY —
//!   restoring the original (keeps the old file) — unless the
//!   re-read is byte-identical to the intended bytes, which is
//!   exactly what proves the non-rewritten tail byte-identical to
//!   the original.
//! - Result JSON: `{"rewritten": "signoff.md",
//!   "warm_start_lines": <n>, "history_blocks_moved": 1,
//!   "tail_bytes_preserved": true}`.

use std::collections::HashMap;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config;
use crate::gate;
use crate::rpc::{Handler, HandlerResult, Server, Tool};

/// The `write_warm_start` tool shape as advertised by `tools/list` —
/// the description is the pinned rewrite/preservation contract (spec
/// §3, verbatim), and the schema is the pinned
/// `{content, orchestrator_token}` shape (contract 50-52).
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "write_warm_start".to_string(),
        description: "Use at the start of the day (or when priorities change) to set the ranked 'if you read nothing else' list that read_signoff hands every session: rewrites ONLY the 'If you read nothing else' warm-start block of signoff.md (signoff.md:5) and moves the superseded block to the 'History' section (signoff.md:119) — implementing the 'warm-start = triage, not a log' rule (CLAUDE.md:9), which otherwise has NO writer. Orchestrator-gated: pass today's claim token from claim_orchestrator; a 'handshake incomplete' refusal means call read_signoff then retry this once. Worker signoff lines are append_signoff's territory, never yours. Every byte outside the rewritten region — all worker-signoff lines, verbatim — is preserved: the server writes a tmp file and renames atomically under the signoff lock, then re-reads and fails LOUDLY (isError, keeps the old file) unless the non-rewritten tail is byte-identical to the original. Server-mediated writes ONLY — direct edits to these files (signoff.md is rewritten by this tool and appended to by append_signoff alone) bypass the lock, the audit trail, and the guard/launcher gates and break the launcher's by-FILE verification.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Full new warm-start block INCLUDING its own '## If you read nothing else' heading line — the server stores content verbatim and moves the superseded block (heading included) to History. Content without that heading on its own line is REFUSED: the reader ranks only the numbered lines UNDER the heading, so a heading-less block would read back as ranked: [] (naive-client study §3.5)"
                },
                "orchestrator_token": {
                    "type": "string",
                    "description": "Token returned by claim_orchestrator for today — the claim file's token, this session's stored token, and this one must all agree"
                }
            },
            "required": ["content", "orchestrator_token"],
            "additionalProperties": false
        }),
    }]
}

/// The dispatch table entry: `tools/call` for `write_warm_start`
/// routes here. The handler names the server's OWN process-local
/// session (spec §2); the unit-level [`write_warm_start`] takes the
/// session explicitly so the gate tests can drive named sessions.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert(
        "write_warm_start".to_string(),
        write_warm_start_handler,
    );
    handlers
}

fn write_warm_start_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    write_warm_start(server, &session, arguments)
}

/// Rewrites the warm-start block of signoff.md (the contract entry
/// point — `session` is the caller's session identity, the key the
/// uniform gate consults). Pinned order — every refusal leaves ZERO
/// bytes written: (1) handshake gate, (2) argument validation,
/// (3) the claim gate — the presented token vs. the claim file's
/// token AND the session's stored token, BEFORE any lock is
/// acquired, (4) the heading contract — content must carry its own
/// `## If you read nothing else` line, since the reader ranks only the
/// numbered lines UNDER that heading and heading-less content would
/// land as a dead block (read_signoff answers ranked: [] — the study's
/// §3.5 symptom), still decided with ZERO disk contact, (5) only then
/// the locked rewrite: read the file under the signoff lock, locate
/// the region, compose the new bytes from the original's own pieces,
/// tmp+rename, re-read, and fail loudly (restoring the original)
/// unless the re-read is byte-identical to the intended bytes.
pub fn write_warm_start(server: &mut Server, session: &str, args: &Value) -> HandlerResult {
    // 1. The uniform gate (spec §4).
    if let Err(msg) = gate::gate(server, session) {
        return HandlerResult::Err(msg);
    }

    // 2. Schema validation (zero side effects).
    let (content, token) = match parse_warm_start_args(args) {
        Ok(v) => v,
        Err(msg) => return HandlerResult::Err(msg),
    };

    let root = server.root.clone();
    let today = match local_date() {
        Ok(d) => d,
        Err(msg) => return HandlerResult::Err(msg),
    };

    // 3. The claim gate (spec §2/§5), decided BEFORE any lock is
    //    taken — a refusal here creates no lock file and rewrites no
    //    byte of signoff.md. BOTH legs must pass: the claim file's
    //    token for today (re-read from disk — never a memory map)
    //    AND the server's per-session claim_token for this caller
    //    (set by the caller's own claim_orchestrator — a claim file
    //    planted on disk alone is not a claim); and the
    //    caller-supplied token must equal the claim file's token.
    let claim_path = root.join(".claims").join(format!("orchestrator-{today}"));
    let file_token = match read_claim(&claim_path) {
        ClaimRead::Held { token, .. } => Some(token),
        ClaimRead::Absent | ClaimRead::Orphan => None,
    };
    let per_session = server.claim_tokens.get(session).cloned();
    let valid = file_token.as_deref() == Some(token.as_str())
        && per_session.as_deref() == Some(token.as_str());
    if !valid {
        return HandlerResult::Err(format!(
            "write_warm_start refused: no valid orchestrator_{today} claim — single-writer warm-start (the claim file's token, the session's stored token, and the presented orchestrator_token must all agree) — nothing written"
        ));
    }

    // 4. The heading contract (naive-client study DEFECT-4): the
    //    content is stored VERBATIM and the reader ranks only the
    //    numbered lines UNDER its `## If you read nothing else`
    //    heading — heading-less content used to land as a dead block
    //    (read_signoff answered ranked: [] after a reported-successful
    //    write). Refuse it loudly, before the lock is acquired.
    if !content.lines().any(|line| line == "## If you read nothing else") {
        return HandlerResult::Err(
            "write_warm_start refused: content must include its own '## If you read nothing else' heading line — the server stores content verbatim and read_signoff ranks only the numbered lines UNDER that heading, so heading-less content lands as a dead block (read_signoff would answer ranked: []) — nothing written".to_string(),
        );
    }

    // 5. The rewrite, under the signoff lock. The lock is acquired
    //    ONLY now — after every refusal above — and is released on
    //    every exit path of the critical section.
    let lock = match acquire_lock(&root, session) {
        Ok(l) => l,
        Err(msg) => return HandlerResult::Err(msg),
    };
    let result = rewrite_under_lock(&root, &content);
    release_lock(&root, &lock);
    match result {
        Ok(mut v) => {
            // F1 (signoffs/review_Wave4OKF.md 2d): the warm-start block
            // HAS landed, so a missing audit tail is reported as an
            // `audit_error` field on the SUCCESS result — never as
            // `isError`, which spec §2 scopes to refusal/validation/
            // gate-denial and which, rendered over landed bytes, is the
            // both-halves FAIL spec §8 pins. When the audit landed the
            // success shape is byte-identical: no `audit_error` key.
            let audit_error =
                crate::tools::ticks::audit_write(&root, &server.session_id, "write_warm_start")
                    .err();
            if let Some(err) = audit_error {
                if let Some(object) = v.as_object_mut() {
                    object.insert("audit_error".to_string(), json!(err));
                }
            }
            HandlerResult::Ok(v)
        }
        Err(msg) => HandlerResult::Err(msg),
    }
}

/// Strict argument parsing: the arguments must be a JSON object with
/// exactly the two schema fields, both strings, and `content` must
/// carry non-whitespace (an empty warm-start block would destroy the
/// region). Refusals name the tool and leave zero side effects.
fn parse_warm_start_args(args: &Value) -> Result<(String, String), String> {
    let obj = match args.as_object() {
        Some(obj) => obj,
        None => {
            return Err(format!(
                "write_warm_start refused: arguments must be a JSON object (got {})",
                value_kind(args)
            ))
        }
    };
    let allowed = ["content", "orchestrator_token"];
    let unknown: Vec<&String> = obj
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "write_warm_start refused: unknown field(s) {unknown:?} — the schema allows only: content, orchestrator_token"
        ));
    }
    let content = require_string(obj, "content")?;
    if content.trim().is_empty() {
        return Err(
            "write_warm_start refused: 'content' is empty — an empty warm-start block would destroy the region — nothing written"
                .to_string(),
        );
    }
    let token = require_string(obj, "orchestrator_token")?;
    Ok((content, token))
}

/// The kind name of a JSON value (serde_json's own wording) — used
/// in refusal messages.
fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// A required schema field: present AND a string.
fn require_string(obj: &Map<String, Value>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        None => Err(format!(
            "write_warm_start refused: missing required field '{key}'"
        )),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "write_warm_start refused: field '{key}' must be a string (got {})",
            value_kind(other)
        )),
    }
}

/// Split a file's text into lines, each element KEEPING its
/// terminating `\n` (the final line may lack one). Unlike
/// `str::lines()` — which strips terminators and drops the empty
/// element after a trailing newline — this split is lossless:
/// concatenating the elements reproduces the input byte-for-byte,
/// and the indices line up 1:1 with `str::lines()` in every other
/// respect (which is what lets the region location mirror the
/// reader's line arithmetic exactly).
fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let take = rest
            .find('\n')
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        out.push(&rest[..take]);
        rest = &rest[take..];
    }
    out
}

/// A raw line with its terminator (and a stray `\r`) stripped, for
/// matching against the reader's exact-line predicates.
fn line_content(line: &str) -> &str {
    let s = line.strip_suffix('\n').unwrap_or(line);
    s.strip_suffix('\r').unwrap_or(s)
}

/// The reader's warm-start region as a contiguous line range
/// `[start, end)` — exactly the wave-2 reader's region
/// (signoff_read `parse_warm_start`): from the
/// `## If you read nothing else` heading, plus the nearest preceding
/// NON-BLANK line when that line is a `Last updated:` line (blank
/// lines are skipped — the real file separates the two with one), up
/// to the next `## ` heading (or the end of the file). `None` when
/// the file has no such heading.
fn locate_region(lines: &[&str]) -> Option<(usize, usize)> {
    let heading = lines
        .iter()
        .position(|line| line_content(line) == "## If you read nothing else")?;
    let end = lines[heading + 1..]
        .iter()
        .position(|line| line_content(line).starts_with("## "))
        .map(|offset| heading + 1 + offset)
        .unwrap_or(lines.len());
    let start = lines[..heading]
        .iter()
        .rev()
        .position(|line| !line_content(line).trim().is_empty())
        .map(|back| heading - 1 - back)
        .filter(|&i| line_content(lines[i]).starts_with("Last updated:"))
        .unwrap_or(heading);
    Some((start, end))
}

/// The `## History` section (signoff_read `parse_history`): the exact
/// `## History` heading line, the section end (the next `## ` heading
/// or EOF), and the insertion point for the superseded block (after
/// the section's last non-blank line, or directly after the heading
/// for an empty section). `Ok(None)` when the file has no `## History`
/// section — a rewrite then creates one at EOF (contract 53-54).
/// `Err` (loud refusal, nothing written) when the section is NOT
/// located entirely after the rewritten region — moving the block
/// there would rewrite pinned bytes (the head or the middle), which
/// the preservation contract forbids.
fn locate_history(
    lines: &[&str],
    region_end: usize,
) -> Result<Option<(usize, usize, usize)>, String> {
    let heading = match lines.iter().position(|line| line_content(line) == "## History") {
        Some(h) => h,
        None => return Ok(None),
    };
    if heading < region_end {
        return Err(
            "write_warm_start refused: the '## History' section is not located after the warm-start region (inside or before it) — moving the superseded block there would rewrite pinned bytes — nothing written"
                .to_string(),
        );
    }
    let end = lines[heading + 1..]
        .iter()
        .position(|line| line_content(line).starts_with("## "))
        .map(|offset| heading + 1 + offset)
        .unwrap_or(lines.len());
    let insert = (heading + 1..end)
        .rev()
        .find(|&i| !line_content(lines[i]).trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(heading + 1);
    Ok(Some((heading, end, insert)))
}

/// The superseded block rendered as History entries: one line per
/// non-blank line of the old region, each prefixed with `- ` and
/// newline-terminated. This is the entry shape the reader's
/// `parse_history` already parses (it strips a leading `- `), and it
/// keeps the section's scan intact — a verbatim `## ` heading line
/// moved into the section would terminate the reader's section scan
/// and orphan the rest of the moved block. Blank lines carry no
/// content and are dropped.
fn render_history_block(region: &[&str]) -> Vec<String> {
    region
        .iter()
        .filter(|line| !line_content(line).trim().is_empty())
        .map(|line| format!("- {}", line_content(line).trim()))
        .collect()
}

/// The rewrite proper, running INSIDE the signoff lock: read the
/// original (under the lock — a concurrent appender cannot interleave
/// between the read and the rename), locate the region, compose the
/// new bytes from the original's own pieces, write a tmp file and
/// rename, re-read, and fail loudly (restoring the original) unless
/// the re-read is byte-identical to the intended bytes.
fn rewrite_under_lock(root: &Path, content: &str) -> Result<Value, String> {
    let path = root.join("signoff.md");
    let original = std::fs::read(&path).map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            "write_warm_start refused: signoff.md is absent — there is no warm-start region to rewrite — nothing written"
                .to_string()
        } else {
            format!("write_warm_start failed: reading signoff.md: {e}")
        }
    })?;
    let original_text = std::str::from_utf8(&original).map_err(|e| {
        format!(
            "write_warm_start refused: signoff.md is not valid UTF-8 ({e}) — the warm-start region cannot be located — nothing written"
        )
    })?;
    let lines = split_lines(&original_text);

    let (start, end) = match locate_region(&lines) {
        Some(r) => r,
        None => {
            return Err(
                "write_warm_start refused: cannot locate the '## If you read nothing else' warm-start region (with its 'Last updated:' header line, when present) in signoff.md — nothing written"
                    .to_string(),
            )
        }
    };
    let hist = locate_history(&lines, end)?;

    // The rewritten region: the caller's block, normalized to end
    // with a newline (a final line without one would fuse with the
    // next byte on re-read).
    let normalized = if content.ends_with('\n') {
        content.to_string()
    } else {
        format!("{content}\n")
    };
    let new_region: Vec<&str> = split_lines(&normalized);

    // The superseded block, rendered for the History section.
    let block = render_history_block(&lines[start..end]);

    // Compose the new file from the original's own pieces:
    //   head + new region + middle + History (up to the insertion
    //   point) + rendered block + remainder
    // so a byte-identical re-read proves every byte OUTSIDE the
    // rewritten region is byte-identical to the original.
    let mut intended = String::with_capacity(original_text.len() + normalized.len());
    for line in &lines[..start] {
        intended.push_str(line);
    }
    for line in &new_region {
        intended.push_str(line);
    }
    match hist {
        Some((h, _h_end, insert)) => {
            for line in &lines[end..h] {
                intended.push_str(line);
            }
            for line in &lines[h..insert] {
                intended.push_str(line);
            }
            for entry in &block {
                intended.push_str(entry);
                intended.push('\n');
            }
            for line in &lines[insert..] {
                intended.push_str(line);
            }
        }
        None => {
            // No History section: the middle runs to EOF, and the
            // section is created at EOF (contract 53-54).
            for line in &lines[end..] {
                intended.push_str(line);
            }
            if !original_text.is_empty() && !original_text.ends_with('\n') {
                intended.push_str("\n## History\n");
            } else {
                intended.push_str("## History\n");
            }
            for entry in &block {
                intended.push_str(entry);
                intended.push('\n');
            }
        }
    }

    // The write: a tmp file in the same directory (same filesystem —
    // the rename is atomic), then renamed over signoff.md.
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".signoff.md.tmp-{}-{seq}", std::process::id());
    let tmp = root.join(&tmp_name);
    if let Err(e) = std::fs::write(&tmp, intended.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write_warm_start failed: writing the tmp signoff file ({tmp_name}): {e}"));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "write_warm_start failed: replacing signoff.md: {e}"
        ));
    }

    // The re-read (still under the lock): the file on disk must be
    // byte-identical to the bytes just written. The intended bytes
    // are composed of the original's own verbatim pieces plus the
    // rewritten region and the rendered superseded block, so a
    // matching re-read proves the pin: every byte outside the
    // rewritten region (the non-rewritten tail — all
    // worker-signoff lines) is byte-identical to the original.
    let re_read = std::fs::read(&path)
        .map_err(|e| format!("write_warm_start failed: re-reading signoff.md after the rename: {e}"))?;
    if re_read != intended.as_bytes() {
        // Something else touched the file between the rename and the
        // re-read (a non-lock-respecting writer, a torn write, a
        // filesystem anomaly): restore the ORIGINAL and fail loudly —
        // the pin keeps the old file.
        let seq2 = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!(".signoff.md.tmp-{}-{seq2}", std::process::id());
        let tmp = root.join(&tmp_name);
        match std::fs::write(&tmp, original.as_slice()).and_then(|_| std::fs::rename(&tmp, &path)) {
            Ok(()) => {}
            Err(e) => eprintln!(
                "[totalrecall] write_warm_start: could not restore the original signoff.md ({e}) — the file on disk is the unverified rewrite"
            ),
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(
            "write_warm_start failed: re-read verification FAILED — signoff.md on disk does not match the intended rewrite (a concurrent writer or filesystem anomaly touched it) — the original file was restored, nothing was changed"
                .to_string(),
        );
    }

    Ok(json!({
        "rewritten": "signoff.md",
        "warm_start_lines": new_region.len(),
        "history_blocks_moved": 1,
        "tail_bytes_preserved": true
    }))
}

/// Monotonic suffix for tmp names (pid + sequence) — no two writers
/// in the same process can collide on a tmp name, and a crashed
/// writer's tmp is identifiable and cleanable.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// The .locks/ signoff mutex (spec §2 concurrency model) — the SAME
// protocol and the SAME lock file as the wave-2 `append_signoff`
// (one naming scheme, not a second one: a rewrite and an append
// serialize on this one lock). The holder record is written INTO the
// lock file; release removes it only while the record still matches.
// ---------------------------------------------------------------------------

/// The signoff.md lock file name (spec §2 — the shared lock every
/// signoff.md writer takes).
const LOCK_NAME: &str = "signoff.md.lock";

/// A lock whose holder has been silent this long is stale: taken over
/// server-side (spec §2 — "a stale lock (its holder process is gone) is
/// taken over"). 30 s is far past any legitimate critical section here
/// (a millisecond-scale rewrite), short against a human noticing.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// Sleep between acquire attempts while a FRESH lock is held (bounded
/// retry, spec §2).
const RETRY_SLEEP: Duration = Duration::from_millis(100);

/// Bounded retry budget for acquire: 50 × 100 ms = 5 s. A holder that
/// neither finishes nor goes stale within the budget is reported loudly
/// with zero bytes written; the next attempt after the holder goes
/// silent is taken over immediately on the next attempt, no budget
/// consumed.
const MAX_ACQUIRE_ATTEMPTS: u32 = 50;

/// One acquisition of the signoff lock file: everything the release step
/// and the staleness detector need. `session` is last in the on-disk
/// contents (it may legally contain whitespace); `pid`/`nonce`/`epoch`
/// are whitespace-delimited and uniquely identify the acquisition.
struct LockToken {
    pid: u32,
    session: String,
    nonce: u64,
    epoch: u64,
}

fn lock_contents(token: &LockToken) -> String {
    format!(
        "pid={} nonce={} epoch={} session={}\n",
        token.pid, token.nonce, token.epoch, token.session
    )
}

/// Monotonic nonce for the per-process lock identity. Each acquire
/// bumps it, so the pid + nonce pair identifies this process's
/// current lock hold even after a previous (crashed) hold of the
/// same pid left a file behind.
static ACQUIRE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Acquire the exclusive lock file (spec §2): race `O_CREAT|O_EXCL` with
/// bounded retry; a stale lock (holder silent ≥ `STALE_AFTER`) is detected
/// by age and taken over server-side (the old file is removed and the
/// create retried immediately); a fresh lock is waited out with
/// `RETRY_SLEEP` until the bounded budget is exhausted, then the rewrite
/// is refused loudly with zero bytes written.
fn acquire_lock(root: &Path, session: &str) -> Result<LockToken, String> {
    let locks_dir = root.join(".locks");
    if let Err(e) = std::fs::create_dir_all(&locks_dir) {
        return Err(format!("write_warm_start refused: cannot create .locks/: {e}"));
    }
    let path = locks_dir.join(LOCK_NAME);
    let pid = std::process::id();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let epoch = match now_epoch() {
            Ok(epoch) => epoch as u64,
            Err(msg) => return Err(msg),
        };
        let nonce = ACQUIRE_NONCE.fetch_add(1, Ordering::Relaxed);
        let token = LockToken {
            pid,
            session: session.to_string(),
            nonce,
            epoch,
        };
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            // O_CREAT|O_EXCL succeeded — we hold the lock. Publish the
            // holder record (crash between create and write leaves an
            // EMPTY lock file; the mtime fallback in `lock_age` still
            // ages it out, so an orphan can never wedge the root).
            Ok(mut f) => {
                let res = f.write_all(lock_contents(&token).as_bytes());
                let flush = f.flush();
                drop(f);
                if res.is_err() || flush.is_err() {
                    // We never published a usable holder record: undo the
                    // bare creation so the file is not left half-owned.
                    let _ = std::fs::remove_file(&path);
                    return Err(format!(
                        "write_warm_start refused: writing .locks/{LOCK_NAME} failed — lock release failed, zero bytes written"
                    ));
                }
                return Ok(token);
            }
            // Someone else holds it.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // F2 (signoffs/review_Wave4OKF.md 2b): age-then-remove
                // unconditionally is a TOCTOU — two acquirers that both
                // measured the same stale record could each remove
                // whatever stood at the path, so the slower one deleted
                // the faster one's FRESH lock and both believed they held
                // the mutex. The takeover is now identity-checked: the
                // removal only ever removes the very record it measured.
                if let Some(stale) = measure_stale_lock(&path) {
                    if remove_stale_lock(&path, &stale) {
                        // Stale: the holder has been silent for
                        // STALE_AFTER and the file still records that
                        // dead holder — take over server-side (spec §2),
                        // no budget spent waiting on a dead holder.
                        continue;
                    }
                    // The re-check refused: a successor took the stale
                    // lock over between the measurement and this
                    // removal. We release our claim on that path (their
                    // lock stays untouched) and re-enter the bounded
                    // retry below.
                }
                if attempts >= MAX_ACQUIRE_ATTEMPTS {
                    return Err(format!(
                        "write_warm_start refused: .locks/{LOCK_NAME} is held by another writer and the bounded retry ({MAX_ACQUIRE_ATTEMPTS} x {RETRY_SLEEP_MS} ms) is exhausted — zero bytes written; retry, or the stale takeover will reclaim it once the holder goes silent",
                        RETRY_SLEEP_MS = RETRY_SLEEP.as_millis()
                    ));
                }
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(e) => {
                return Err(format!(
                    "write_warm_start refused: acquiring .locks/{LOCK_NAME}: {e}"
                ))
            }
        }
    }
}

/// How long the current holder has been silent: the holder's own
/// `epoch` (recorded at acquisition) when the lock file is readable and
/// parseable, else the file's mtime (covers a crash that left the file
/// empty), else `None` — an age that cannot be established is treated as
/// FRESH: no takeover, just the bounded wait (the safe default).
fn lock_age(path: &Path) -> Option<Duration> {
    let now = SystemTime::now();
    if let Ok(contents) = std::fs::read_to_string(path) {
        if let Some(raw) = parse_lock_field(&contents, "epoch") {
            if let Ok(holder_epoch) = raw.parse::<u64>() {
                if let Ok(now_since) = now.duration_since(UNIX_EPOCH) {
                    if holder_epoch <= now_since.as_secs() {
                        return Some(Duration::from_secs(
                            now_since.as_secs() - holder_epoch,
                        ));
                    }
                }
            }
        }
    }
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(modified) => now.duration_since(modified).ok(),
        Err(_) => None,
    }
}

/// The identity of the lock file as measured STALE (F2,
/// signoffs/review_Wave4OKF.md 2b): the published `pid`/`nonce`/`epoch`
/// fields — the same fields `release_lock` identity-checks — the full
/// contents, and the stat facts (mtime, size, inode) at measurement
/// time. `None` fields cover the empty/corrupt file whose age came from
/// the mtime fallback (its measured identity is then its stat alone).
struct StaleLock {
    pid: Option<String>,
    nonce: Option<String>,
    epoch: Option<String>,
    contents: Option<String>,
    mtime: Option<SystemTime>,
    size: u64,
    ino: u64,
}

/// Measure the holder's silence AND its identity in one step: `Some`
/// only when the file exists, its age (the `lock_age` rule: published
/// `epoch`, else mtime) is at least `STALE_AFTER`, and its stat facts
/// could be recorded. `None` means FRESH or unmeasurable — the caller
/// waits (the safe default).
fn measure_stale_lock(path: &Path) -> Option<StaleLock> {
    let age = lock_age(path)?;
    if age < STALE_AFTER {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let contents = std::fs::read_to_string(path).ok();
    let field = |name: &str| contents.as_deref().and_then(|c| parse_lock_field(c, name));
    Some(StaleLock {
        pid: field("pid"),
        nonce: field("nonce"),
        epoch: field("epoch"),
        contents,
        mtime: meta.modified().ok(),
        size: meta.len(),
        ino: meta.ino(),
    })
}

/// The identity-safe takeover (F2): re-read the record and re-stat the
/// file, and remove it ONLY while every identity field (pid/nonce/
/// epoch) and every stat fact (mtime/size/inode) still matches what was
/// measured stale — a mismatch means someone took the lock over first,
/// and removing then would delete THEIR fresh lock. Returns whether the
/// removal was ours to make. Residual window: between this verified
/// re-read and the `remove_file` itself microseconds remain open for a
/// successor to land — recorded as the accepted residual by the review
/// (F2); fully closing it needs rename-based takeover, out of this
/// round's scope.
fn remove_stale_lock(path: &Path, stale: &StaleLock) -> bool {
    let now_contents = std::fs::read_to_string(path).ok();
    if now_contents != stale.contents {
        return false;
    }
    let text = now_contents.as_deref().unwrap_or("");
    if [
        ("pid", &stale.pid),
        ("nonce", &stale.nonce),
        ("epoch", &stale.epoch),
    ]
    .iter()
    .any(|(name, want)| parse_lock_field(text, name) != **want)
    {
        return false;
    }
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if meta.len() != stale.size || meta.ino() != stale.ino || meta.modified().ok() != stale.mtime
    {
        return false;
    }
    std::fs::remove_file(path).is_ok()
}

/// Parse a whitespace-delimited `key=value` field from lock contents.
fn parse_lock_field(contents: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    contents
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix(&prefix))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

/// Release: remove OUR lock file (spec §2 — "release = the server removes
/// its own lock file"). The file is re-read first and only removed when
/// its `pid`/`nonce`/`epoch` still match this token — if the lock was
/// stale-taken-over out from under us, its successor's file is left
/// alone. Release failures are logged, never fatal: the write already
/// landed, and a stranded lock ages out and is reclaimed.
fn release_lock(root: &Path, token: &LockToken) {
    let path = root.join(".locks").join(LOCK_NAME);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return, // already gone: nothing of ours to release
    };
    let matches = [
        (
            parse_lock_field(&contents, "pid"),
            Some(token.pid.to_string()),
        ),
        (
            parse_lock_field(&contents, "nonce"),
            Some(token.nonce.to_string()),
        ),
        (
            parse_lock_field(&contents, "epoch"),
            Some(token.epoch.to_string()),
        ),
    ];
    if !matches.iter().all(|(a, b)| a.as_deref() == b.as_deref()) {
        eprintln!(
            "[totalrecall] write_warm_start: .locks/{LOCK_NAME} changed under us (holder record no longer matches) — leaving it for its current holder"
        );
        return;
    }
    if let Err(e) = std::fs::remove_file(&path) {
        eprintln!(
            "[totalrecall] write_warm_start: could not remove our .locks/{LOCK_NAME} ({e}) — the stale takeover will reclaim it"
        );
    }
}

/// The state of a claim record on disk (spec §5): the file is the
/// truth. `Held` when present and carrying all three fields
/// (`holder_session_id`, `token`, `acquired_at`) — the same
/// usability rule the claims module applies, so a record one module
/// treats as an orphan is refused here too; `Absent` when there is no
/// claim file for the date; `Orphan` when a file is present but is
/// not a usable claim (empty, unreadable, malformed JSON, or missing
/// any field).
enum ClaimRead {
    Absent,
    Held { token: String },
    Orphan,
}

fn read_claim(path: &Path) -> ClaimRead {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return ClaimRead::Absent,
        Err(_) => return ClaimRead::Orphan, // present but unreadable
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return ClaimRead::Orphan,
    };
    let holder = v.get("holder_session_id").and_then(Value::as_str);
    let token = v.get("token").and_then(Value::as_str);
    let acquired_at = v.get("acquired_at").and_then(Value::as_str);
    match (holder, token, acquired_at) {
        (Some(_), Some(t), Some(_)) => ClaimRead::Held {
            token: t.to_string(),
        },
        _ => ClaimRead::Orphan,
    }
}

/// The server's LOCAL date (`YYYY-MM-DD`), derived the way the gate
/// derives it (the contract: "derived the way the gate does from the
/// root's tz fingerprint" — the gate's own `local_date` uses
/// `config::resolve_tz()`, which is keyed to the root's fingerprint
/// at init): the epoch clock shifted by the timezone offset, and
/// Howard Hinnant's civil-from-days integer calendar math (the crate
/// ships no date dependency).
fn local_date() -> Result<String, String> {
    let (_, offset) = config::resolve_tz()
        .map_err(|e| format!("cannot resolve the server's timezone: {e}"))?;
    let now = match now_epoch() {
        Ok(secs) => secs + offset * 60,
        Err(msg) => return Err(msg),
    };
    let days = if now >= 0 { now / 86_400 } else { (now - 86_399) / 86_400 };
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = era * 400 + yoe;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let (year, m) = if mp < 10 { (y, mp + 3) } else { (y + 1, mp - 9) };
    Ok(format!("{year:04}-{:02}-{:02}", m, day))
}

/// Seconds since the Unix epoch, `Err` (refuse loudly) when the
/// system clock predates the epoch — a negative stamp would
/// silently mislabel day boundaries.
fn now_epoch() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|_| {
            "the system clock is before the Unix epoch — refusing to stamp (refuse loudly)"
                .to_string()
        })
}
