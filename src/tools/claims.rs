//! `claim_orchestrator` + `write_dayfile` (wave 3, spec §3/§5) — the
//! single-writer orchestrator claim and the day-file writer it gates.
//!
//! Pinned semantics (wave-3 contract; spec §3/§4/§5/§8):
//! - The on-disk claim file `.claims/orchestrator-<date>` (JSON
//!   `{holder_session_id, token, acquired_at}`) is the SOURCE OF
//!   TRUTH (spec §5): the server re-reads it on every call, never a
//!   memory map. A fresh claim is created with `O_CREAT|O_EXCL` and
//!   returns a token. A caller whose session id matches the recorded
//!   holder gets the EXISTING token back — a re-claim after a server
//!   restart succeeds, because the truth is on disk. A different
//!   session on a live claim is refused with the holder and reason
//!   named, and the claim file is left byte-identical.
//! - `claim_orchestrator` is handshake-gated like every other
//!   disk-writing tool (spec §4 GATED set): the gate runs FIRST —
//!   the exact pinned refusal, the attempt is counted, zero disk. A
//!   non-handshaken session cannot claim; the wave-3 gate's claim
//!   fixture handshakes its sessions before claiming for exactly
//!   this reason.
//! - A successful claim ALSO stores the token in the server's
//!   per-session state (`Server::claim_tokens`, spec §2's
//!   `{session_id, handshaked, claim_token}`) keyed by the CALLER'S
//!   session — process-local, re-stored by a re-claim after a
//!   restart. This is what `write_brief` validates (its schema has
//!   no token param) and what `write_dayfile` / `write_warm_start`
//!   co-validate alongside the claim file's token.
//! - `date` defaults to the server's LOCAL date (spec §2: the
//!   zone/offset the root's init fingerprint records); every emitted
//!   timestamp is UTC with a trailing `Z`.
//! - `write_dayfile` is gated on the handshake (spec §4) AND on
//!   today's claim with BOTH token checks (spec §2/§3/§5): the
//!   caller-supplied `orchestrator_token` must equal the token
//!   recorded in the claim file under the day file's LOCAL date AND
//!   the server's per-session claim_token for the caller's session —
//!   a claim file planted on disk alone is NOT a valid claim. Any
//!   other outcome — no claim, an orphan claim, a mismatched or
//!   missing per-session token — is the exact pinned refusal
//!   `write_dayfile refused: no valid orchestrator_<date> claim —
//!   single-writer dayfile`, decided BEFORE any lock is acquired.
//! - On success the day file `YYYY-MM-DD.md` is replaced byte-exact
//!   and atomically (tmp file + rename, both in the root) under the
//!   day file's own exclusive `.locks/` lock — the wave-2 lock
//!   mechanism, verbatim; only the lock file name differs.
//! - Stale-claim rotation (a claim dated BEFORE the server's local
//!   date moves to `.claims/archive/`) is internal housekeeping that
//!   runs inside `claim_orchestrator` — not an API surface (spec
//!   §3 EXCLUSIONS).
//! - Every refusal leaves ZERO bytes written: the gate refusal and
//!   the schema refusals touch no disk at all, the token refusal is
//!   decided before any lock is acquired, and the lock's bounded
//!   retry writes nothing when exhausted (spec §8: "a server that
//!   says `isError` and writes anyway FAILS").

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::gate;
use crate::rpc::{Handler, HandlerResult, Server, Tool};

/// The claim bucket (spec §2 storage table): one claim file per date,
/// `orchestrator-<date>` — the claim source of truth.
const CLAIMS_DIR: &str = ".claims";

/// Where a claim dated before the server's local date retires (spec
/// §5 housekeeping — "stale-claim rotation"; internal, not API).
const ARCHIVE_DIR: &str = ".claims/archive";

/// A claim file that exists but is unreadable (an orphan: a crash
/// between the `O_CREAT|O_EXCL` and the JSON write, or a corrupted
/// file) is taken over server-side once it has been orphaned this
/// long — the same rule the `.locks/` protocol applies (spec §2: the
/// orphan can never wedge the root).
const STALE_AFTER: Duration = Duration::from_secs(30);

/// Sleep between day-file-lock acquire attempts while a FRESH lock is
/// held (spec §2: bounded retry, then refuse loudly).
const RETRY_SLEEP: Duration = Duration::from_millis(100);

/// Bounded retry budget for the day-file lock acquire: 50 × 100 ms =
/// 5 s, then a loud refusal (a check that can't run must fail
/// loudly — spec §8).
const MAX_ACQUIRE_ATTEMPTS: u32 = 50;

/// Per-process acquire sequence: makes each acquisition's lock
/// contents unique even for two acquisitions from the same
/// pid+session in the same second, so "remove your own lock file" can
/// never remove a successor's. The same counter names the per-call
/// tmp file (`.<date>.md.tmp-<pid>-<seq>`), so tmp names are unique
/// per process too.
static ACQUIRE_NONCE: AtomicU64 = AtomicU64::new(0);

/// The tool shapes this module exposes (spec §3, verbatim).
pub fn tools() -> Vec<Tool> {
    vec![claim_orchestrator_tool(), write_dayfile_tool()]
}

/// The dispatch entries this module owns. In the live server the
/// session IS the process-local `server.session_id` (spec §2:
/// identity is process-local), so the adapters pass it through; the
/// unit-level functions take the session explicitly so the gate tests
/// can drive named sessions.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert(
        "claim_orchestrator".to_string(),
        claim_orchestrator_handler,
    );
    handlers.insert("write_dayfile".to_string(), write_dayfile_handler);
    handlers
}

fn claim_orchestrator_tool() -> Tool {
    Tool {
        name: "claim_orchestrator".to_string(),
        description: "Call this before the first orchestrator write of the day — write_dayfile / write_brief / write_warm_start all require the token it returns. Claims orchestrator-ship for a date; the on-disk claim file .claims/orchestrator-<date> is the source of truth (§5): the server re-reads it on every call, never a memory map. Fresh claim: O_CREAT|O_EXCL, returns a token. This call is itself handshake-gated: call read_signoff FIRST — a 'handshake incomplete — call read_signoff first' answer means call read_signoff, then retry this once. Session ids are server-generated per server process and never client-supplied, so a NEW server process is a NEW session and CANNOT re-claim — there is no re-claim path after a server restart. Different session on a live claim: refused — an isError result naming the holder and reason; if the refusal names another holder, do NOT poll or retry: report the named holder session to the human. Planting a claim file by hand is NOT a claim — the claim files are tool-written only.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "date": {"type": "string", "description": "YYYY-MM-DD; defaults to the server's LOCAL date (§2) — omit to claim today"}
            },
            "additionalProperties": false
        }),
    }
}

fn write_dayfile_tool() -> Tool {
    Tool {
        name: "write_dayfile".to_string(),
        description: "Use this — never a hand edit — to record the day: writes today's day file (YYYY-MM-DD.md, named by the server's LOCAL date, §2) as a SINGLE-WRITER whole-file replace. Pass `orchestrator_token` exactly as today's claim_orchestrator returned it. REFUSES without today's valid orchestrator token ('write_dayfile refused: no valid orchestrator_<date> claim — single-writer dayfile') — call claim_orchestrator once for the day first; if THAT refusal names another holder, stop and report to the human — repeat write_dayfile calls can never land while another session holds the claim; a 'handshake incomplete' refusal means call read_signoff then retry this once. The day files under the memory root are written ONLY through this tool: direct edits bypass the claim gate, the append lock and the audit trail. Never call concurrently with another orchestrator.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Full new day-file content (server replaces the file; no merge)"},
                "orchestrator_token": {"type": "string", "description": "Token returned by claim_orchestrator for today — pass it verbatim; an absent or mismatched token is refused, there is no fallback"}
            },
            "required": ["content", "orchestrator_token"],
            "additionalProperties": false
        }),
    }
}

fn claim_orchestrator_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    claim_orchestrator(server, &session, arguments)
}

fn write_dayfile_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    write_dayfile(server, &session, arguments)
}

/// The server's LOCAL date for this root as `YYYY-MM-DD` (spec §2:
/// "the day-file name (YYYY-MM-DD.md) and the claim date use the
/// server's local date"). The root is single-timezone by construction:
/// the init fingerprint (`.state/init.json`, written by
/// `config::init_state`) records the zone + UTC offset at first init,
/// and the server refuses to start if a later start resolves a
/// different one — so the RECORDED offset is the date authority and
/// the per-call resolution is pure arithmetic (no `date` subprocess
/// per call). A root whose fingerprint is missing (a test that built
/// the server without init) falls back to resolving the zone live —
/// the same call `init_state` would have recorded.
pub fn local_date(root: &Path) -> Result<String, String> {
    let offset = match read_recorded_offset(root) {
        Some(offset) => offset,
        None => {
            let (_, offset) = crate::config::resolve_tz()
                .map_err(|e| format!("cannot resolve the server's timezone: {e}"))?;
            offset
        }
    };
    let secs = now_epoch()?;
    // Local wall-clock = UTC now + the recorded offset; the date is
    // the first ten characters of the crate's shared UTC renderer
    // applied to that instant (reuses the pinned calendar
    // arithmetic).
    let stamp = gate::utc_iso8601(secs as i64 + offset * 60);
    Ok(stamp[..10].to_string())
}

/// The `tz_offset_minutes` recorded in the root's init fingerprint,
/// `None` when the fingerprint is absent or unreadable (the caller
/// then resolves the zone live).
fn read_recorded_offset(root: &Path) -> Option<i64> {
    let text = std::fs::read_to_string(root.join(".state").join("init.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("tz_offset_minutes").and_then(|x| x.as_i64())
}

/// Current epoch seconds. A pre-epoch system clock is a
/// misconfiguration the server refuses loudly rather than stamping
/// with a nonsense time.
fn now_epoch() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| {
            "the system clock is before the Unix epoch — refusing to stamp (refuse loudly)".to_string()
        })
}

/// Validate an explicit `date` as a real calendar date in `YYYY-MM-DD`
/// (pure string/integer checks — no third-party date dependency, spec
/// §2). `YYYY-MM-DD` names compare chronologically as strings, so a
/// validated name is safe to use in file names and in `date < today`
/// comparisons.
fn is_real_date(date: &str) -> bool {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    // `str::parse` for ints TRIMS whitespace (" 2" parses as 2) — the
    // digit check is the real gate.
    if !(0..10).all(|i| i == 4 || i == 7 || b[i].is_ascii_digit()) {
        return false;
    }
    let Ok(year) = date[0..4].parse::<u32>() else {
        return false;
    };
    let Ok(month) = date[5..7].parse::<u32>() else {
        return false;
    };
    let Ok(day) = date[8..10].parse::<u32>() else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    (1..=days_in_month(year, month)).contains(&day)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
        _ => 0,
    }
}

/// Claims orchestrator-ship for a date (the contract entry point —
/// `session` is the caller's session identity, the key the uniform
/// gate consults).
///
/// The on-disk claim file is the source of truth (spec §5): every
/// call re-reads `.claims/orchestrator-<date>`; nothing claim-shaped
/// is kept in memory. Order of operations, pinned so every refusal
/// leaves the claim file byte-identical (spec §8):
/// 1. handshake gate (spec §4) — refuse + count, no disk at all;
/// 2. argument validation (`date` optional; when present it must be a
///    real `YYYY-MM-DD` — the default is the server's LOCAL date) —
///    refuse, no disk at all;
/// 3. stale-claim rotation (spec §5 housekeeping): claims dated
///    before the local date move to `.claims/archive/`;
/// 4. the claim itself: absent → create with `O_CREAT|O_EXCL` and
///    return the new token; held by THIS session → return the
///    EXISTING token (idempotent re-claim — it survives a server
///    restart); held by ANOTHER session → refuse, naming holder +
///    reason, file byte-identical. Every SUCCESSFUL claim or
///    re-claim also stores the token in the server's per-session
///    state (spec §2) keyed by the caller's session — the state
///    `write_brief` validates and `write_dayfile` / `write_warm_start`
///    co-validate; refused claims store nothing.
pub fn claim_orchestrator(server: &mut Server, session: &str, args: &Value) -> HandlerResult {
    // 1. The uniform gate (spec §4) — claim_orchestrator is a
    //    disk-writing tool, so it gates exactly like every other
    //    one: the exact pinned refusal, the attempt is counted so
    //    `session_compliance` can observe it, and nothing
    //    claim-shaped touches any disk.
    if let Err(e) = gate::gate(server, session) {
        return HandlerResult::Err(e);
    }

    // 2. Validate before touching any disk (zero side effects).
    let obj = match args.as_object() {
        Some(obj) => obj,
        None => {
            return HandlerResult::Err(
                "claim_orchestrator refused: arguments must be a JSON object".to_string(),
            )
        }
    };
    let allowed = ["date"];
    let unknown: Vec<&String> = obj.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        return HandlerResult::Err(format!(
            "claim_orchestrator refused: unknown field(s) {:?} — the schema allows only: date",
            unknown
        ));
    }
    let requested: Option<String> = match obj.get("date") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => {
            return HandlerResult::Err(format!(
                "claim_orchestrator refused: field 'date' must be a string when present (got {})",
                value_kind(other)
            ))
        }
    };
    let today = match local_date(&server.root) {
        Ok(today) => today,
        Err(msg) => return HandlerResult::Err(msg),
    };
    if let Some(d) = requested.as_ref() {
        if !is_real_date(d) {
            return HandlerResult::Err(format!(
                "claim_orchestrator refused: date '{d}' is not a valid YYYY-MM-DD calendar date"
            ));
        }
    }

    // 3. Internal housekeeping (spec §5): a claim dated before the
    //    server's local date is stale and retires to the archive.
    //    Rotation failures are logged, never fatal — they cannot
    //    affect the requested date's own file.
    let root = server.root.clone();
    rotate_stale_claims(&root, &today);

    // The date is resolved here — after the gate (step 1), the
    // validation (step 2) and the rotation (step 3). `today` is moved
    // on purpose: the rotation was its last use, and the validation
    // ran before any disk, so a bad `date` refuses with zero side
    // effects.
    let date = match requested {
        Some(d) => d,
        None => today,
    };

    // 4. The claim, decided from the DISK on every call.
    let claims_dir = root.join(CLAIMS_DIR);
    let path = claims_dir.join(format!("orchestrator-{date}"));
    match read_claim(&path) {
        // Absent: the fresh-claim path — race the create with
        // O_CREAT|O_EXCL (a lost race re-reads below).
        ClaimRead::Absent => {
            let mut res = create_fresh_claim(&claims_dir, &path, session, &date);
            // The per-session claim_token (spec §2), keyed by the
            // caller's session (the gate key) — this is what
            // `write_brief` validates and what `write_dayfile` /
            // `write_warm_start` co-validate. A lost race that
            // resolves back to THIS session stores the winner's
            // token the same way (idempotency under a race).
            if let HandlerResult::Ok(v) = &mut res {
                if let Some(t) = v.get("token").and_then(Value::as_str) {
                    server.claim_tokens.insert(session.to_string(), t.to_string());
                }
                // F1 (signoffs/review_Wave4OKF.md 2d): the claim HAS
                // landed, so a missing audit tail is reported as an
                // `audit_error` field on the SUCCESS result — never as
                // `isError`, which spec §2 scopes to refusal/validation/
                // gate-denial and which, rendered over landed bytes, is
                // the both-halves FAIL spec §8 pins. When the audit
                // landed the success shape is byte-identical: no key.
                if v.get("claimed").and_then(Value::as_bool) == Some(true) {
                    if let Some(err) = crate::tools::ticks::audit_write(
                        &server.root,
                        &server.session_id,
                        "claim_orchestrator",
                    )
                    .err()
                    {
                        if let Some(object) = v.as_object_mut() {
                            object.insert("audit_error".to_string(), json!(err));
                        }
                    }
                }
            }
            res
        }
        // Present: the file is the truth.
        ClaimRead::Held {
            holder,
            token,
            acquired_at,
        } => {
            if holder == session {
                // Idempotent re-claim: the EXISTING token is returned
                // (not a new one), so a server restart that loses the
                // process-local state re-claims cleanly — and the
                // per-session claim_token is re-stored from the disk
                // truth (spec §2). The file is NOT rewritten —
                // byte-identical.
                server.claim_tokens.insert(session.to_string(), token.clone());
                // F1 (signoffs/review_Wave4OKF.md 2d): the held claim is
                // the disk truth and stays landed — a missing audit tail
                // rides the success result as `audit_error`, never as
                // `isError` over landed bytes (spec §2/§8). With the
                // audit landed, the shape is byte-identical: no key.
                let audit_error = crate::tools::ticks::audit_write(
                    &server.root,
                    &server.session_id,
                    "claim_orchestrator",
                )
                .err();
                let mut result = json!({
                    "claimed": true,
                    "reclaimed": true,
                    "date": date,
                    "holder": holder,
                    "token": token,
                    "acquired_at": acquired_at,
                });
                if let Some(err) = audit_error {
                    if let Some(object) = result.as_object_mut() {
                        object.insert("audit_error".to_string(), json!(err));
                    }
                }
                HandlerResult::Ok(result)
            } else {
                // A live claim held by another session: refused, the
                // holder and reason are named, the file is left
                // byte-identical, and the server's per-session state
                // is untouched (a refused claim stores nothing).
                HandlerResult::Err(claim_conflict_refusal(&date, &holder, &acquired_at))
            }
        }
        // Present but unreadable: an orphan (a crash between the
        // O_CREAT|O_EXCL and the JSON write) or a corrupted file.
        // Same rule as the .locks/ protocol (spec §2): an orphan is
        // taken over server-side once it ages out; a FRESH orphan is
        // waited out loudly rather than raced.
        ClaimRead::Orphan => match claim_age(&path) {
            Some(a) if a >= STALE_AFTER => {
                let _ = std::fs::remove_file(&path);
                eprintln!(
                    "[totalrecall] claim_orchestrator: .claims/orchestrator-{date} was orphaned (holder crashed before publishing) and is {a:?} old — taken over server-side"
                );
                let mut res = create_fresh_claim(&claims_dir, &path, session, &date);
                // A takeover makes THIS session the holder: the
                // fresh token is stored per-session like any other
                // successful claim (spec §2).
                if let HandlerResult::Ok(v) = &mut res {
                    if let Some(t) = v.get("token").and_then(Value::as_str) {
                        server.claim_tokens.insert(session.to_string(), t.to_string());
                    }
                    // F1 (signoffs/review_Wave4OKF.md 2d): the takeover
                    // claim HAS landed — an audit tail that did not land
                    // is an `audit_error` field on the success, never
                    // `isError` over landed bytes (spec §2/§8). No key
                    // at all when the audit landed.
                    if v.get("claimed").and_then(Value::as_bool) == Some(true) {
                        if let Some(err) = crate::tools::ticks::audit_write(
                            &server.root,
                            &server.session_id,
                            "claim_orchestrator",
                        )
                        .err()
                        {
                            if let Some(object) = v.as_object_mut() {
                                object.insert("audit_error".to_string(), json!(err));
                            }
                        }
                    }
                }
                res
            }
            _ => HandlerResult::Err(format!(
                "claim_orchestrator refused: .claims/orchestrator-{date} exists but is not a readable claim (an orphan from a crashed holder) — it will be taken over once it has been orphaned for {} s; retry, or delete the file",
                STALE_AFTER.as_secs()
            )),
        },
    }
}

/// The refusal a lost claim race (or a live claim) produces: the
/// holder is named and the rule restated, and NOTHING is written —
/// the claim file stays byte-identical (spec §5/§8).
fn claim_conflict_refusal(date: &str, holder: &str, acquired_at: &str) -> String {
    format!(
        "claim_orchestrator refused: orchestrator-{date} is held by session={holder} (acquired {acquired_at}) — a live claim is single-writer: only the holding session may claim that date. The claim file is unchanged; the holder must release (or the claim must age out) before another session may take it"
    )
}

/// What a claim-file read found. `Held` carries exactly the fields
/// the on-disk JSON carries (the disk truth the decision is made
/// from); `Orphan` means the file is present but is not a valid
/// claim (empty / unreadable / malformed JSON / missing fields).
enum ClaimRead {
    Absent,
    Held {
        holder: String,
        token: String,
        acquired_at: String,
    },
    Orphan,
}

/// Read the claim file (the source of truth — never a memory map,
/// spec §5). A `NotFound` read is `Absent`; any other failure or a
/// file that does not carry all three fields is `Orphan`.
fn read_claim(path: &Path) -> ClaimRead {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ClaimRead::Absent,
        Err(_) => return ClaimRead::Orphan, // present but unreadable
    };
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return ClaimRead::Orphan,
    };
    let holder = v
        .get("holder_session_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let token = v.get("token").and_then(|x| x.as_str()).map(|s| s.to_string());
    let acquired_at = v
        .get("acquired_at")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    match (holder, token, acquired_at) {
        (Some(h), Some(t), Some(a)) => ClaimRead::Held {
            holder: h,
            token: t,
            acquired_at: a,
        },
        _ => ClaimRead::Orphan,
    }
}

/// How long an orphan claim file has been on disk: its mtime (the
/// orphan published no epoch of its own — the crash happened between
/// the create and the write). `None` when the age cannot be
/// established: treated as FRESH (no takeover, just wait) — the safe
/// default, the same rule the `.locks/` protocol applies.
fn claim_age(path: &Path) -> Option<Duration> {
    let now = SystemTime::now();
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(modified) => now.duration_since(modified).ok(),
        Err(_) => None,
    }
}

/// The fresh-claim critical path: create `.claims/`, race
/// `O_CREAT|O_EXCL` on the claim file (spec §5), then publish
/// `{holder_session_id, token, acquired_at}`. A LOST race (another
/// session created the file between our read and our create)
/// re-reads the file: if we lost to OUR OWN session the winner's
/// token is returned (idempotency under a race), otherwise the
/// refusal names the winner. A failed publish undoes the bare create
/// so no half-owned file is left behind.
fn create_fresh_claim(
    claims_dir: &Path,
    path: &Path,
    session: &str,
    date: &str,
) -> HandlerResult {
    if let Err(e) = std::fs::create_dir_all(claims_dir) {
        return HandlerResult::Err(format!(
            "claim_orchestrator refused: cannot create .claims/: {e}"
        ));
    }
    let epoch = match now_epoch() {
        Ok(epoch) => epoch,
        Err(msg) => return HandlerResult::Err(msg),
    };
    let nonce = ACQUIRE_NONCE.fetch_add(1, Ordering::Relaxed);
    // A token unique to this (pid, nonce): enough entropy for the
    // single-writer gate, cheap to mint.
    let token = format!(
        "tok-{:016x}",
        ((std::process::id() as u64).wrapping_mul(0x9E37_79B9) ^ epoch)
            .wrapping_mul(nonce + 1)
    );
    let acquired_at = gate::utc_iso8601(epoch as i64);
    match std::fs::OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut f) => {
            let doc = json!({
                "holder_session_id": session,
                "token": token,
                "acquired_at": acquired_at,
            });
            let res = f.write_all(doc.to_string().as_bytes());
            let flush = f.flush();
            drop(f);
            if res.is_err() || flush.is_err() {
                // We never published a readable claim: undo the bare
                // creation so the file is not left half-owned.
                let _ = std::fs::remove_file(path);
                return HandlerResult::Err(format!(
                    "claim_orchestrator refused: writing .claims/orchestrator-{date} failed — the create was undone, no claim recorded"
                ));
            }
            HandlerResult::Ok(json!({
                "claimed": true,
                "reclaimed": false,
                "date": date,
                "holder": session,
                "token": token,
                "acquired_at": acquired_at,
            }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Lost the race: the file appeared between our read and
            // our create. The file is the truth — re-read it.
            match read_claim(path) {
                ClaimRead::Held {
                    holder,
                    token,
                    acquired_at,
                } if holder == session => HandlerResult::Ok(json!({
                    "claimed": true,
                    "reclaimed": true,
                    "date": date,
                    "holder": holder,
                    "token": token,
                    "acquired_at": acquired_at,
                })),
                other => HandlerResult::Err(match other {
                    ClaimRead::Held {
                        holder,
                        acquired_at,
                        ..
                    } => claim_conflict_refusal(date, &holder, &acquired_at),
                    // The file vanished or stayed unreadable between
                    // our create and our re-read — retry.
                    ClaimRead::Absent | ClaimRead::Orphan => format!(
                        "claim_orchestrator refused: .claims/orchestrator-{date} appeared between our read and our create but is not a usable claim — retry"
                    ),
                }),
            }
        }
        Err(e) => HandlerResult::Err(format!(
            "claim_orchestrator refused: creating .claims/orchestrator-{date}: {e}"
        )),
    }
}

/// Stale-claim rotation (spec §5, internal housekeeping — NOT an API
/// surface): every claim dated strictly before the server's local
/// date moves from `.claims/` to `.claims/archive/`. `YYYY-MM-DD`
/// names compare chronologically as strings, so the comparison is a
/// string compare. An archive name that already exists is never
/// overwritten (that file's rotation is skipped and logged). Rotation
/// is best-effort: it is housekeeping for a bucket this very call is
/// about to write, and a failure here is logged, not fatal (the
/// requested date's own claim is untouched either way).
fn rotate_stale_claims(root: &Path, today: &str) {
    let claims_dir = root.join(CLAIMS_DIR);
    let Ok(entries) = std::fs::read_dir(&claims_dir) else {
        return; // no claims yet: nothing to rotate
    };
    let archive_dir = root.join(ARCHIVE_DIR);
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(date) = name.as_ref().strip_prefix("orchestrator-") else {
            continue; // not a claim file (e.g. the archive/ subdir)
        };
        if !is_real_date(date) || date >= today {
            continue; // valid and current-or-future: stays
        }
        if let Err(e) = std::fs::create_dir_all(&archive_dir) {
            eprintln!("[totalrecall] claim_orchestrator: cannot create .claims/archive/: {e} — stale claim {name} left in place");
            continue;
        }
        let target = archive_dir.join(&file_name);
        if target.exists() {
            eprintln!("[totalrecall] claim_orchestrator: .claims/archive/{name} already exists — the stale claim is left in place (archives are never overwritten)");
            continue;
        }
        if let Err(e) = std::fs::rename(&entry.path(), &target) {
            eprintln!("[totalrecall] claim_orchestrator: could not rotate stale claim .claims/{name} to the archive: {e} — left in place");
        }
    }
}

/// Writes today's day file (the contract entry point — `session` is
/// the caller's session identity).
///
/// Order of operations, pinned so every refusal leaves ZERO bytes
/// written (spec §8):
/// 1. handshake gate (spec §4) — refuse + count, no disk at all;
/// 2. argument validation (both schema fields required strings) —
///    refuse, no disk at all;
/// 3. the token gate (spec §2/§3/§5) — BOTH legs, and only then:
///    the caller-supplied `orchestrator_token` must equal the token
///    recorded in `.claims/orchestrator-<today>` (the disk is
///    re-read — never a memory map) AND the server's per-session
///    claim_token for the caller's session (set by that session's
///    own `claim_orchestrator`). Any other outcome is the exact
///    pinned refusal `write_dayfile refused: no valid
///    orchestrator_<date> claim — single-writer dayfile`, decided
///    BEFORE any lock is acquired, so the refusal creates neither
///    lock file nor day file;
/// 4. the write itself, under the day file's exclusive `.locks/` lock
///    (spec §2 concurrency model): tmp file + atomic rename, then
///    release.
pub fn write_dayfile(server: &mut Server, session: &str, args: &Value) -> HandlerResult {
    // 1. The uniform gate (spec §4).
    if let Err(msg) = gate::gate(server, session) {
        return HandlerResult::Err(msg);
    }

    // 2. Schema validation (zero side effects).
    let obj = match args.as_object() {
        Some(obj) => obj,
        None => {
            return HandlerResult::Err("write_dayfile refused: arguments must be a JSON object".to_string())
        }
    };
    let allowed = ["content", "orchestrator_token"];
    let unknown: Vec<&String> = obj.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    if !unknown.is_empty() {
        return HandlerResult::Err(format!(
            "write_dayfile refused: unknown field(s) {:?} — the schema allows only: content, orchestrator_token",
            unknown
        ));
    }
    let content = match require_string(obj, "content") {
        Ok(c) => c,
        Err(msg) => return HandlerResult::Err(msg),
    };
    let token = match require_string(obj, "orchestrator_token") {
        Ok(t) => t,
        Err(msg) => return HandlerResult::Err(msg),
    };

    let root = server.root.clone();
    let today = match local_date(&root) {
        Ok(t) => t,
        Err(msg) => return HandlerResult::Err(msg),
    };

    // 3. The single-writer token gate (spec §2/§5), decided BEFORE
    //    any lock is taken — a refusal here creates no lock file
    //    and no day file. BOTH legs must pass: the claim file's
    //    token for today (re-read from disk — never a memory map)
    //    AND the server's per-session claim_token for this caller
    //    (set by the caller's own claim_orchestrator — a claim
    //    file planted on disk alone is not a claim).
    let claim_path = root.join(CLAIMS_DIR).join(format!("orchestrator-{today}"));
    let file_token = match read_claim(&claim_path) {
        ClaimRead::Held { token, .. } => Some(token),
        ClaimRead::Absent | ClaimRead::Orphan => None,
    };
    let per_session = server.claim_tokens.get(session).cloned();
    let valid = file_token.as_deref() == Some(token.as_str())
        && per_session.as_deref() == Some(token.as_str());
    if !valid {
        return HandlerResult::Err(format!(
            "write_dayfile refused: no valid orchestrator_{today} claim — single-writer dayfile"
        ));
    }

    // 4. The replacement, atomic under the day file's own lock
    //    (spec §2 — the wave-2 `.locks/` protocol, same mechanism,
    //    the day file's own lock name).
    let lock_name = format!("{today}.md.lock");
    match replace_dayfile_under_lock(&root, session, &today, &lock_name, &content) {
        Ok(()) => {
            // F1 (signoffs/review_Wave4OKF.md 2d): the day file HAS
            // landed, so a missing audit tail is reported as an
            // `audit_error` field on the SUCCESS result — never as
            // `isError`, which spec §2 scopes to refusal/validation/
            // gate-denial and which, rendered over landed bytes, is the
            // both-halves FAIL spec §8 pins. When the audit landed the
            // success shape is byte-identical: no `audit_error` key.
            let audit_error =
                crate::tools::ticks::audit_write(&root, &server.session_id, "write_dayfile").err();
            let mut result = json!({
                "replaced": true,
                "path": root.join(format!("{today}.md")).display().to_string(),
                "date": today,
                "bytes": content.len(),
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

/// Hold the day-file lock across the whole replacement; the release
/// runs on every exit (a failed write must not wedge the lock for
/// `STALE_AFTER`).
fn replace_dayfile_under_lock(
    root: &Path,
    session: &str,
    date: &str,
    lock_name: &str,
    content: &str,
) -> Result<(), String> {
    let token = acquire_lock(root, session, lock_name)?;
    let result = write_dayfile_under_lock(root, date, content, token.nonce);
    release_lock(root, &token, lock_name);
    result
}

/// The critical section: write the new content to a per-call tmp
/// file IN THE ROOT (the same directory — and therefore the same
/// filesystem — as the target, so the rename is atomic), then rename
/// it over the day file. A rename failure or a crash leaves the
/// previous bytes in place; the tmp file is removed on every failure
/// path. The tmp name carries this acquisition's nonce, so it is
/// unique per process.
fn write_dayfile_under_lock(root: &Path, date: &str, content: &str, nonce: u64) -> Result<(), String> {
    let path = root.join(format!("{date}.md"));
    let tmp = root.join(format!(
        ".{date}.md.tmp-{}-{nonce}",
        std::process::id()
    ));
    if let Err(err) = std::fs::write(&tmp, content.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "write_dayfile refused: writing the tmp day file: {err} — the previous {date}.md is untouched"
        ));
    }
    if let Err(err) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "write_dayfile refused: renaming the tmp day file into {date}.md: {err} — the previous day file is untouched"
        ));
    }
    Ok(())
}

/// One acquisition of the day-file lock file: everything the release
/// step and the staleness detector need. `session` is last in the
/// on-disk contents (it may legally contain whitespace);
/// `pid`/`nonce`/`epoch` are whitespace-delimited and uniquely
/// identify the acquisition.
struct LockToken {
    pid: u32,
    session: String,
    nonce: u64,
    epoch: u64,
}

/// The exact on-disk bytes for one lock file: the whitespace-delimited
/// identity fields first, the session (which may legally contain
/// whitespace) LAST. The parser reads the named prefix, so the
/// session's contents can never be confused with a field.
fn lock_contents(token: &LockToken) -> String {
    format!(
        "pid={} nonce={} epoch={} session={}\n",
        token.pid, token.nonce, token.epoch, token.session
    )
}

/// Acquire the exclusive lock file (spec §2): race `O_CREAT|O_EXCL`
/// with bounded retry; a stale lock (holder silent ≥ `STALE_AFTER`)
/// is detected by age and taken over server-side (the old file is
/// removed and the create retried immediately); a fresh lock is
/// waited out with `RETRY_SLEEP` until the bounded budget is
/// exhausted, then the write is refused loudly with zero bytes
/// written.
///
/// This is the wave-2 mechanism, verbatim — the same `.locks/`
/// scheme, contents format, and retry/stale rules; only the lock file
/// name differs (the day file has its own mutex: `YYYY-MM-DD.md.lock`),
/// so day-file writes are serialized per day file and never against
/// the signoff's lock.
fn acquire_lock(root: &Path, session: &str, lock_name: &str) -> Result<LockToken, String> {
    let locks_dir = root.join(".locks");
    if let Err(e) = std::fs::create_dir_all(&locks_dir) {
        return Err(format!(
            "write_dayfile refused: cannot create .locks/: {e}"
        ));
    }
    let path = locks_dir.join(lock_name);
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
        match std::fs::OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut f) => {
                let res = f.write_all(lock_contents(&token).as_bytes());
                let flush = f.flush();
                drop(f);
                if res.is_err() || flush.is_err() {
                    // The lock file was created but we could not
                    // publish our identity into it: undo it, else
                    // release (and the staleness detector) would see
                    // an unparseable file we nominally "own".
                    let _ = std::fs::remove_file(&path);
                    return Err(format!(
                        "write_dayfile refused: writing .locks/{lock_name} failed — the lock was not taken, zero bytes written"
                    ));
                }
                return Ok(token);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // The file is the truth: a concurrent holder owns it.
                // F2 (signoffs/review_Wave4OKF.md 2b): age-then-remove
                // unconditionally is a TOCTOU — two acquirers that both
                // measured the same stale record could each remove
                // whatever stood at the path, so the slower one deleted
                // the faster one's FRESH lock and both believed they held
                // the mutex. The takeover is now identity-checked: the
                // removal only ever removes the very record it measured.
                if let Some(stale) = measure_stale_lock(&path) {
                    if remove_stale_lock(&path, &stale) {
                        // STALE holder (silent for ≥ STALE_AFTER) and
                        // still the recorded one: spec §2 mandates the
                        // take over — an orphaned lock must never wedge
                        // the root.
                        continue; // retry the create immediately
                    }
                    // The re-check refused: a successor took the stale
                    // lock over between the measurement and this
                    // removal. We release our claim on that path (their
                    // lock stays untouched) and re-enter the bounded
                    // retry below.
                }
                if attempts >= MAX_ACQUIRE_ATTEMPTS {
                    return Err(format!(
                        "write_dayfile refused: .locks/{lock_name} is held by another writer and the bounded retry ({MAX_ACQUIRE_ATTEMPTS} x {retries} ms) is exhausted — zero bytes written; retry, or the stale takeover will reclaim it once the holder goes silent",
                        retries = RETRY_SLEEP.as_millis()
                    ));
                }
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(e) => {
                return Err(format!(
                    "write_dayfile refused: acquiring .locks/{lock_name}: {e}"
                ))
            }
        }
    }
}

/// How long a lock file has been held: the age of its published
/// `epoch` (UTC Z), with the mtime as a fallback for a corrupt file.
/// `None` when the age cannot be established: the caller treats that
/// as a FRESH lock (no takeover, just wait) — the safe default,
/// since a wrong guess on an unreadable file would let two writers
/// believe they hold the lock.
fn lock_age(path: &Path) -> Option<Duration> {
    let now = SystemTime::now();
    let content = std::fs::read_to_string(path).ok()?;
    let since = if let Some(holder_epoch) = parse_lock_field(&content, "epoch").and_then(|e| e.parse::<u64>().ok()) {
        UNIX_EPOCH + Duration::from_secs(holder_epoch)
    } else {
        // The epoch field is missing/unreadable: fall back to the
        // file's mtime.
        std::fs::metadata(path).ok()?.modified().ok()?
    };
    now.duration_since(since).ok()
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
    let field = |name: &str| {
        contents
            .as_deref()
            .and_then(|c| parse_lock_field(c, name).map(str::to_string))
    };
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
    .any(|(name, want)| parse_lock_field(text, name).map(str::to_string) != **want)
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

/// Extract a whitespace-delimited `name=value` field from lock-file
/// contents. Only the named prefix is parsed — everything after the
/// value (including the `session` field, whose value may legally
/// contain whitespace) is ignored.
fn parse_lock_field<'a>(content: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=");
    let start = content.find(&needle)? + needle.len();
    let rest = &content[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let v = &rest[..end];
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Release: remove OUR lock file (spec §2 — "release = the server
/// removes its own lock file"). The file is re-read first and only
/// removed when its `pid`/`nonce`/`epoch` still match this token —
/// if the lock was stale-taken-over out from under us, its
/// successor's file is left alone. Release failures are logged,
/// never fatal: the write already landed, and a stranded lock ages
/// out and is reclaimed (spec §2).
fn release_lock(root: &Path, token: &LockToken, lock_name: &str) {
    let path = root.join(".locks").join(lock_name);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return, // nothing to release (or gone)
    };
    let ours = [
        ("pid", token.pid.to_string()),
        ("nonce", token.nonce.to_string()),
        ("epoch", token.epoch.to_string()),
    ]
    .iter()
    .all(|(name, want)| parse_lock_field(&content, name).map(|got| got == want).unwrap_or(false));
    if !ours {
        eprintln!(
            "[totalrecall] write_dayfile: .locks/{lock_name} no longer records this acquisition — not removed (a successor's lock)"
        );
        return;
    }
    if let Err(e) = std::fs::remove_file(&path) {
        eprintln!("[totalrecall] write_dayfile: could not release .locks/{lock_name}: {e} — it will be reclaimed once stale");
    }
}

/// A required string argument, or the house refusal message (which
/// never touches disk — schema validation happens before any file is
/// read or written).
fn require_string(obj: &Map<String, Value>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        None => Err(format!("write_dayfile refused: missing required field '{key}'")),
        Some(other) => Err(format!(
            "write_dayfile refused: field '{key}' must be a string (got {})",
            value_kind(other)
        )),
    }
}

/// A short human-readable kind for a non-string JSON value (the
/// refusal names what it got, not a stack trace).
fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
