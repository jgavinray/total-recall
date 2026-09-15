//! `append_signoff` — the verbatim one-line worker signoff (wave 2, spec §3).
//!
//! Pinned semantics (wave-2 contract; spec §3/§4/§7):
//! - The tool emits EXACTLY ONE line to `signoff.md`:
//!   `Worker signoff (<role>) | done: <yes|no> | unpushed: <…> | awaits human: <…> | still running: <…>`
//!   then the optional `| kaibo review: <…>` (passed through verbatim — the
//!   server NEVER synthesizes it), then the server stamps
//!   `| workflow: <…> | ts: <ISO-8601Z> | session: <…>`. That order is the
//!   on-disk format the shipped guard and launcher already parse.
//! - The uniform handshake gate (spec §4) applies: a pre-handshake call is
//!   refused with the exact pinned text and counted in
//!   `attempted_write_before_handshake`, with ZERO side effects (no lock
//!   file, no write).
//! - A serialized entry (incl. server stamps) > 16384 bytes is refused with
//!   zero bytes written; the cap clears the measured longest real entry
//!   (4919 B at signoff.md:356, measured 2026-09-13).
//! - The append runs under an exclusive `O_CREAT|O_EXCL` lock file in
//!   `.locks/` (spec §2 concurrency model): bounded-retry acquire; a stale
//!   lock (holder silent for `STALE_AFTER`) is detected by age and taken
//!   over server-side; release = the server removes its own lock file.
//!   `O_APPEND` positions the write; no bare-atomicity claim above the cap.
//! - Pre-existing bytes of `signoff.md` are preserved verbatim; when the
//!   file does not end with a newline the server inserts one before the
//!   entry so the entry is always exactly one line. A fresh root's
//!   `signoff.md` is created on first write (spec §2 storage table).
//! - The audit-log write (`.audit/`, spec §2) is a later wave: wave 2
//!   touches `signoff.md` and `.locks/` only.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::os::unix::fs::MetadataExt;

use serde_json::{json, Map, Value};

use crate::gate;
use crate::rpc::{Handler, HandlerResult, Server, Tool};

/// Serialized-entry cap in bytes (spec §3): the measured longest real entry
/// is 4919 B, so 16384 B is size-independent — the LOCK, not a `PIPE_BUF`
/// atomicity assumption, carries the write (spec §2).
const ENTRY_CAP_BYTES: u64 = 16_384;

/// The append mutex on `signoff.md` (spec §2 `.locks/` row): one lock file
/// name shared by every signoff.md writer, so later-wave writers
/// (`write_warm_start`) and this tool serialize on the same file.
const LOCK_NAME: &str = "signoff.md.lock";

/// A lock whose holder has been silent this long is stale: taken over
/// server-side (spec §2 — "a stale lock (its holder process is gone) is
/// detected by age and taken over server-side"). Generous against the
/// millisecond-scale critical section, short against a human noticing.
const STALE_AFTER: Duration = Duration::from_secs(30);

/// Sleep between acquire attempts while a FRESH lock is held (bounded
/// retry, spec §2).
const RETRY_SLEEP: Duration = Duration::from_millis(100);

/// Bounded retry budget for acquire: 50 × 100 ms = 5 s. A holder that
/// neither finishes nor goes stale within the budget is reported loudly
/// rather than waited out forever ("a check that can't run must fail
/// loudly"); the caller may retry — and a lock that has since gone stale
/// is taken over immediately on the next attempt, no budget consumed.
const MAX_ACQUIRE_ATTEMPTS: u32 = 50;

/// Per-process acquire sequence: makes each acquisition's lock contents
/// unique even for two acquisitions from the same pid+session in the same
/// second, so "remove your own lock file" can never remove a successor's.
static ACQUIRE_NONCE: AtomicU64 = AtomicU64::new(0);

/// The registry entry (spec §3, verbatim — description carries the
/// MANDATORY contract text; the input schema is the pinned shape).
fn append_signoff_tool() -> Tool {
    Tool {
        name: "append_signoff".to_string(),
        description: "Last action before any stop/compact/handoff: appends this session's signoff verbatim to signoff.md as exactly ONE line: `Worker signoff (<role>) | done: <yes|no> | unpushed: <…> | awaits human: <…> | still running: <…>` — the on-disk format the shipped guard and launcher already parse — with server-appended `| workflow: … | ts: <ISO-8601Z> | session: …` fields only AFTER the required ones, and `| kaibo review: …` when the optional field is present. APPEND-ONLY by construction under an exclusive `.locks/` lock file; entries (serialized form, incl. server stamps) > 16384 bytes refused — the cap clears the measured longest real entry, 4919 bytes at signoff.md:356 (measured 2026-09-13). REFUSES with 'handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).' unless read_signoff succeeded this session — call read_signoff, then retry this once; if read_signoff answers 'cannot read', that is provisioning, not call order: report it, do not retry. Server-mediated writes ONLY — direct edits to these files (signoff.md is written by this tool and write_warm_start alone) bypass the lock, the audit trail, and the guard/launcher gates and break the launcher's by-FILE verification and the guard's schema check; to read past signoffs use `recall`.".to_string(),
        input_schema: json!({
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
        }),
    }
}

/// The tools this module exposes (the orchestrator-owned registry merges
/// these).
pub fn tools() -> Vec<Tool> {
    vec![append_signoff_tool()]
}

/// The dispatch entries this module owns: the `tools/call` entry point for
/// `append_signoff`. In the live server the session IS the process-local
/// `server.session_id` (spec §2: identity is process-local — the harness
/// exports no session-id env var), so the adapter passes it through.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert("append_signoff".to_string(), append_signoff_handler);
    handlers
}

/// The `tools/call` dispatch entry (see [`handlers`]).
fn append_signoff_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    // Copy the session id out first: the call below mutably borrows
    // `server`, so the session reference must not alias it.
    let session = server.session_id.clone();
    append_signoff(server, &session, arguments)
}

/// Append this session's signoff verbatim to `signoff.md` as exactly ONE
/// line (the contract entry point — `session` is the caller's session
/// identity, the key the uniform gate consults).
///
/// Order of operations, pinned so that every refusal leaves ZERO bytes
/// written (spec §8: "a server that says `isError` and writes anyway
/// FAILS"):
/// 1. handshake gate (spec §4) — refuse + count, no disk at all;
/// 2. argument validation (schema + the one-line invariants) — refuse,
///    no disk at all;
/// 3. serialize the entry; over the 16384-byte cap → refuse, no disk;
/// 4. acquire the `.locks/` lock file (bounded retry, stale takeover),
///    append under it, release it.
pub fn append_signoff(server: &mut Server, session: &str, args: &Value) -> HandlerResult {
    // 1. The uniform gate (spec §4) — same shape as every other gated
    //    tool: exact pinned refusal, the attempt is counted so
    //    `session_compliance` can observe it, and the write never lands.
    if let Err(msg) = gate::gate(server, session) {
        return HandlerResult::Err(msg);
    }

    // 2. Validate before touching any disk (refusals here have no
    //    side effect either).
    let (role, workflow, done, unpushed, awaits_human, still_running, kaibo_review) =
        match parse_args(args) {
            Ok(fields) => fields,
            Err(msg) => return HandlerResult::Err(msg),
        };

    // 3. Serialize the entry (the one line), then apply the cap to the
    //    serialized form INCLUDING the server stamps.
    let epoch = match now_epoch() {
        Ok(epoch) => epoch,
        Err(msg) => return HandlerResult::Err(msg),
    };
    let ts = utc_iso8601(epoch);
    let line = serialize_line(
        &role,
        &done,
        &unpushed,
        &awaits_human,
        &still_running,
        kaibo_review.as_deref(),
        &workflow,
        &ts,
        &server.session_id,
    );
    if line.len() as u64 > ENTRY_CAP_BYTES {
        return HandlerResult::Err(format!(
            "append_signoff refused: serialized entry is {} bytes — the cap is 16384 bytes (it clears the measured longest real entry, 4919 B); zero bytes written",
            line.len()
        ));
    }

    // 4. The append itself, under the exclusive lock (spec §2).
    let root = server.root.clone();
    match perform_append(&root, session, &line) {
        Ok(()) => {
            // F1 (signoffs/review_Wave4OKF.md 2d): the line HAS landed,
            // so a missing audit tail is reported as an `audit_error`
            // field on the SUCCESS result — never as `isError`, which
            // spec §2 scopes to refusal/validation/gate-denial and
            // which, rendered over landed bytes, is the both-halves
            // FAIL spec §8 pins. When the audit landed the success
            // shape is byte-identical to before: no `audit_error` key.
            let audit_error =
                crate::tools::ticks::audit_write(&root, &server.session_id, "append_signoff").err();
            let mut result = json!({
                "appended": true,
                "path": root.join("signoff.md").display().to_string(),
                "entry": line,
            });
            if let Some(err) = audit_error {
                if let Some(object) = result.as_object_mut() {
                    object.insert("audit_error".to_string(), json!(err));
                }
            }
            HandlerResult::Ok(result)
        }
        Err(msg) => HandlerResult::Err(msg),
    }
}

/// The pinned line format (spec §3 "Serialized entry"): the fixed token,
/// the four status fields in guard order, the optional `| kaibo review:`
/// verbatim, then the server stamps — `workflow` first (after the
/// optional field), then `ts` (ISO-8601Z), then the process-local
/// `session`.
fn serialize_line(
    role: &str,
    done: &str,
    unpushed: &str,
    awaits_human: &str,
    still_running: &str,
    kaibo_review: Option<&str>,
    workflow: &str,
    ts: &str,
    session: &str,
) -> String {
    let mut line = format!(
        "Worker signoff ({role}) | done: {done} | unpushed: {unpushed} | awaits human: {awaits_human} | still running: {still_running}"
    );
    if let Some(kaibo_review) = kaibo_review {
        line.push_str(" | kaibo review: ");
        line.push_str(kaibo_review);
    }
    line.push_str(" | workflow: ");
    line.push_str(workflow);
    line.push_str(" | ts: ");
    line.push_str(ts);
    line.push_str(" | session: ");
    line.push_str(session);
    line
}

/// Schema validation (spec §3 `append_signoff` schema) plus the
/// format invariants the schema cannot express: the entry is exactly ONE
/// line, and `role` must keep the fixed token `Worker signoff (<role>)`
/// well-formed (no parentheses, no line breaks, not empty).
fn parse_args(
    args: &Value,
) -> Result<(String, String, String, String, String, String, Option<String>), String> {
    let obj = args
        .as_object()
        .ok_or_else(|| "append_signoff refused: arguments must be a JSON object".to_string())?;

    // additionalProperties: false — unknown fields are refused, named.
    let allowed = [
        "role",
        "workflow",
        "done",
        "unpushed",
        "awaits_human",
        "still_running",
        "kaibo_review",
    ];
    let unknown: Vec<&String> = obj.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "append_signoff refused: unknown field(s) {:?} — the schema allows only: {}",
            unknown,
            allowed.join(", ")
        ));
    }

    let role = require_string(obj, "role")?;
    let workflow = require_string(obj, "workflow")?;
    let done = require_string(obj, "done")?;
    let unpushed = optional_string(obj, "unpushed")?.unwrap_or_else(|| "none".to_string());
    let awaits_human = optional_string(obj, "awaits_human")?
        .unwrap_or_else(|| "none".to_string());
    let still_running = optional_string(obj, "still_running")?
        .unwrap_or_else(|| "no".to_string());
    let kaibo_review = optional_string(obj, "kaibo_review")?;

    if role.trim().is_empty() {
        return Err("append_signoff refused: 'role' must not be empty — it fills the fixed token `Worker signoff (<role>)`".to_string());
    }
    if role.contains('(') || role.contains(')') {
        return Err("append_signoff refused: 'role' may not contain '(' or ')' — the fixed token is `Worker signoff (<role>)`".to_string());
    }
    if done != "yes" && done != "no" {
        return Err(format!("append_signoff refused: 'done' must be \"yes\" or \"no\" (schema enum; got {done:?})"));
    }
    for (field, value) in [
        ("role", role.as_str()),
        ("workflow", workflow.as_str()),
        ("done", done.as_str()),
        ("unpushed", unpushed.as_str()),
        ("awaits_human", awaits_human.as_str()),
        ("still_running", still_running.as_str()),
        ("kaibo_review", kaibo_review.as_deref().unwrap_or("")),
    ] {
        if value.contains('\n') || value.contains('\r') {
            return Err(format!("append_signoff refused: '{field}' contains a newline — the entry is exactly ONE line"));
        }
    }

    Ok((
        role, workflow, done, unpushed, awaits_human, still_running, kaibo_review,
    ))
}

/// The kind name of a JSON value (serde_json's own wording) — used in
/// refusal messages.
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
        None => Err(format!("append_signoff refused: missing required field '{key}'")),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "append_signoff refused: field '{key}' must be a string (got {})",
            value_kind(other)
        )),
    }
}

/// An optional schema field: absent is fine (the caller's default applies
/// at serialization); present but not a string is refused.
fn optional_string(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match obj.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(format!(
            "append_signoff refused: field '{key}' must be a string when present (got {})",
            value_kind(other)
        )),
    }
}

// ---------------------------------------------------------------------------
// The .locks/ append mutex (spec §2 concurrency model)
// ---------------------------------------------------------------------------

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

/// Acquire the exclusive lock file (spec §2): race `O_CREAT|O_EXCL` with
/// bounded retry; a stale lock (holder silent ≥ `STALE_AFTER`) is detected
/// by age and taken over server-side (the old file is removed and the
/// create retried immediately); a fresh lock is waited out with
/// `RETRY_SLEEP` until the bounded budget is exhausted, then the append is
/// refused loudly with zero bytes written.
fn acquire_lock(root: &Path, session: &str) -> Result<LockToken, String> {
    let locks_dir = root.join(".locks");
    if let Err(e) = std::fs::create_dir_all(&locks_dir) {
        return Err(format!("append_signoff refused: cannot create .locks/: {e}"));
    }
    let path = locks_dir.join(LOCK_NAME);
    let pid = std::process::id();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let epoch = match now_epoch() {
            Ok(epoch) => epoch,
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
                        "append_signoff refused: writing .locks/{LOCK_NAME} failed — lock release failed, zero bytes written"
                    ));
                }
                return Ok(token);
            }
            // Someone else holds it.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
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
                        // dead holder — take over (spec §2), no budget
                        // spent waiting on a dead holder.
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
                        "append_signoff refused: .locks/{LOCK_NAME} is held by another writer and the bounded retry ({MAX_ACQUIRE_ATTEMPTS} x {RETRY_SLEEP_MS} ms) is exhausted — zero bytes written; retry, or the stale takeover will reclaim it once the holder goes silent",
                        RETRY_SLEEP_MS = RETRY_SLEEP.as_millis()
                    ));
                }
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(e) => {
                return Err(format!(
                    "append_signoff refused: acquiring .locks/{LOCK_NAME}: {e}"
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
            "[total-recall] append_signoff: .locks/{LOCK_NAME} changed under us (holder record no longer matches) — leaving it for its current holder"
        );
        return;
    }
    if let Err(e) = std::fs::remove_file(&path) {
        eprintln!(
            "[total-recall] append_signoff: could not remove our .locks/{LOCK_NAME} ({e}) — the stale takeover will reclaim it"
        );
    }
}

/// Hold the lock across the whole append; the release runs on every
/// exit (a failed write must not wedge the lock for STALE_AFTER).
fn perform_append(root: &Path, session: &str, line: &str) -> Result<(), String> {
    let token = acquire_lock(root, session)?;
    let result = append_under_lock(root, line);
    release_lock(root, &token);
    result
}

/// The critical section: pre-existing bytes are preserved verbatim; if
/// the file does not end with a newline one is inserted before the entry
/// (insertion, not modification — existing bytes are untouched); the
/// payload is a single `O_APPEND` write.
fn append_under_lock(root: &Path, line: &str) -> Result<(), String> {
    let path = root.join("signoff.md");

    // Created-on-first-write (spec §2 storage table): a fresh root starts
    // empty, so the first append scaffolds the section the entry belongs
    // to. An EXISTING file is never restructured — those bytes are the
    // other writers' and are preserved verbatim.
    if !path.exists() {
        let scaffold = "# Signoff\n\n## Worker sessions — sign off here as you go\n\n";
        std::fs::write(&path, scaffold).map_err(|e| {
            format!("append_signoff failed: creating signoff.md: {e}")
        })?;
    }

    // We hold the exclusive signoff lock — in wave 2 no other writer
    // touches signoff.md, so this peek cannot race.
    if last_byte_is_not_newline(&path)? {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("append_signoff failed: opening signoff.md: {e}"))?;
        f.write_all(format!("\n{line}\n").as_bytes())
            .and_then(|()| f.flush())
            .map_err(|e| format!("append_signoff failed: writing signoff.md: {e}"))?;
    } else {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("append_signoff failed: opening signoff.md: {e}"))?;
        f.write_all(format!("{line}\n").as_bytes())
            .and_then(|()| f.flush())
            .map_err(|e| format!("append_signoff failed: writing signoff.md: {e}"))?;
    }
    Ok(())
}

/// True when the file exists, is non-empty, and its last byte is not a
/// newline (i.e. a separator must be inserted before the new entry).
fn last_byte_is_not_newline(path: &Path) -> Result<bool, String> {
    let size = std::fs::metadata(path)
        .and_then(|m| Ok(m.len()))
        .map_err(|e| format!("append_signoff failed: stat signoff.md: {e}"))?;
    if size == 0 {
        return Ok(false);
    }
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("append_signoff failed: opening signoff.md: {e}"))?;
    f.seek(SeekFrom::End(-1))
        .map_err(|e| format!("append_signoff failed: seeking signoff.md: {e}"))?;
    let mut b = [0u8; 1];
    f.read_exact(&mut b)
        .map_err(|e| format!("append_signoff failed: reading signoff.md: {e}"))?;
    Ok(b[0] != b'\n')
}

// ---------------------------------------------------------------------------
// UTC clock (spec §2: "all emitted timestamps are UTC with a trailing Z")
// ---------------------------------------------------------------------------

/// Current epoch seconds. A pre-epoch system clock is a misconfiguration
/// the server refuses loudly rather than stamping with a nonsense time.
fn now_epoch() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| {
            format!("append_signoff failed: system clock is before the Unix epoch ({e})")
        })
}

/// `YYYY-MM-DDTHH:MM:SSZ` from epoch seconds — the same convention as
/// config.rs's init-fingerprint `created_at` (pure-integer calendar math,
/// no third-party date dependency per spec §2).
fn utc_iso8601(epoch: u64) -> String {
    let secs = epoch as i64;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, rem / 60 % 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        month, day, hour, minute, second
    )
}

/// Howard Hinnant's `civil_from_days`: pure-integer calendar arithmetic
/// for the UTC date (the same algorithm as config.rs's private copy —
/// the dependency set is std + serde_json only, spec §2). Days are
/// counted from the Unix epoch; internally they are re-based to the
/// March-anchored proleptic Gregorian calendar (719468 days between
/// 0000-03-01 and 1970-01-01) and divided into 400-year eras
/// (146097 days — the calendar's exact repetition period), so the
/// century leap rules fall out of the integer math.
fn civil_from_days(d: i64) -> (i64, u32, u32) {
    let z = d + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097; // floor
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let y = era * 400 + yoe;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let (y, month) = if mp < 10 {
        (y, mp + 3) // March..December stay in the same year
    } else {
        (y + 1, mp - 9) // January/February belong to the next year
    };
    (y, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plant(dir: &Path, pid: u32, nonce: u64, epoch: u64, session: &str) -> std::path::PathBuf {
        let locks = dir.join(".locks");
        std::fs::create_dir_all(&locks).unwrap();
        let path = locks.join(LOCK_NAME);
        std::fs::write(
            &path,
            format!("pid={pid} nonce={nonce} epoch={epoch} session={session}\n"),
        )
        .unwrap();
        path
    }

    /// F2 (signoffs/review_Wave4OKF.md 2b), the review's race replayed
    /// at the helper level, deterministically: acquirer B measures the
    /// dead holder's record STALE; acquirer A takes the lock over (a
    /// FRESH record appears at the same path) BEFORE B's removal. The
    /// pre-fix code removed whatever stood at the path on the strength
    /// of B's stale read alone — deleting A's fresh lock and letting
    /// both hold the mutex. The identity-checked takeover must refuse
    /// A's lock and leave it on disk.
    #[test]
    fn stale_takeover_never_removes_a_successors_fresh_lock() {
        let dir = std::env::temp_dir().join(format!(
            "total-recall-f2-signoff-{}-takeover",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

        // B measures the dead holder: silent for STALE_AFTER+1.
        let path = plant(&dir, 1, 1, now - STALE_AFTER.as_secs() - 1, "s-dead");
        let stale = measure_stale_lock(&path).expect("the dead holder must measure as stale");

        // A takes over in the meantime: the SAME path now carries a
        // fresh record that B never measured.
        std::fs::write(&path, format!("pid=2 nonce=2 epoch={now} session=s-successor\n")).unwrap();

        assert!(
            !remove_stale_lock(&path, &stale),
            "a record that is not the one measured stale is never removed"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("the successor's lock survives the refused takeover"),
            format!("pid=2 nonce=2 epoch={now} session=s-successor\n"),
            "the successor's fresh lock is left byte-identical"
        );
        // A fresh record is not even measurable as stale: the caller
        // re-enters the bounded retry rather than racing again.
        assert!(measure_stale_lock(&path).is_none(), "a fresh lock never re-enters the takeover path");

        // The legitimate takeover still works: the file B measured is
        // still the file on disk — removal is ours to make.
        let path = plant(&dir, 3, 3, now - STALE_AFTER.as_secs() - 1, "s-dead-again");
        let stale = measure_stale_lock(&path).expect("the dead holder must measure as stale");
        assert!(remove_stale_lock(&path, &stale), "the measured stale record IS removable");
        assert!(!path.exists(), "the takeover removed exactly the file it measured");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
