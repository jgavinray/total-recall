//! The `write_brief` tool (spec §3) — the kickoff-brief writer.
//!
//! Pinned semantics (wave-3 contract lines 39-49; spec §2/§3/§4/§5/§8):
//! - Orchestrator-gated. The uniform handshake gate (spec §4) runs
//!   first — a pre-handshake refusal is counted and touches no disk
//!   at all. Then today's claim: the schema is token-less
//!   (`{worker, brief}`), so "requires this session holds today's
//!   claim token" is validated per contract lines 39-43 as the match
//!   between the caller's per-session stored token
//!   (`Server::claim_tokens`, populated by that session's own
//!   `claim_orchestrator`) and the `token` the on-disk claim record
//!   `.claims/orchestrator-<local-date>` carries. The claim file is
//!   the disk truth (spec §5); the per-session map is process-local
//!   and is re-stored by re-claiming after a restart — a claim file
//!   planted on disk alone is NOT a claim.
//! - Pinned order — every refusal leaves ZERO bytes on disk (spec §8):
//!   1. the handshake gate (no disk at all);
//!   2. argument validation (strict `{worker, brief}`; unknown
//!      fields refused);
//!   3. `worker` safety — a single safe path segment (no `/`, no
//!      `..`, non-empty) — BEFORE the `briefs/` bucket is created;
//!   4. the claim gate (the per-session token AND the claim file's
//!      token — both must agree);
//!   5. only then disk work.
//! - Archival (spec §3, verbatim description): the superseded
//!   `briefs/<worker>.md` is archived to
//!   `briefs/<worker>-<superseded-date>T<HHMMSS>Z.md` where the stamp
//!   is the UTC second-granularity rendering OF THE SUPERSEDED
//!   CONTENT'S mtime — the age of the content at the moment it is
//!   superseded. Two supersessions can only collide when the
//!   superseded files share an mtime second (two briefs written in
//!   the same wall-clock second); if the target archive name already
//!   exists with different content, the tool refuses and writes
//!   nothing — no archive is ever overwritten. A byte-identical
//!   re-arrival (same mtime second, same bytes) is idempotent: the
//!   existing archive is kept and re-reported.
//! - The first write for a worker supersedes nothing: no archive
//!   (`archived_to` is `null`), and the `briefs/` bucket is created
//!   only at this point — a refused call never creates it.
//! - Result JSON: `{"path": "briefs/<worker>.md",
//!   "archived_to": "briefs/<worker>-…Z.md" | null}`.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config;
use crate::gate;
use crate::rpc::{Handler, HandlerResult, Server, Tool};

/// The `write_brief` tool shape as advertised by `tools/list` — the
/// description is the pinned archive contract (spec §3, verbatim),
/// and the schema is the pinned token-less `{worker, brief}` shape
/// (contract 39-43).
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "write_brief".to_string(),
        description: "Publish or update ONE worker's kickoff brief (the orchestrator's tool — workers never call it for their own brief): writes briefs/<worker>.md, archiving the superseded version to briefs/<worker>-<superseded-date>T<HHMMSS>Z.md — the UTC timestamp OF THE SUPERSEDED CONTENT (its file mtime, rendered in UTC), so two writes for one worker on one day can never produce the same archive name and no archive is ever overwritten. ORCHESTRATOR-ONLY: this session must hold today's claim token (call claim_orchestrator first; the schema takes no token — the server matches your session against the on-disk claim). REFUSED with 'nothing written' when the claim is missing or held elsewhere; a 'handshake incomplete' refusal means call read_signoff then retry this once. briefs/ is written ONLY through this tool — hand-editing a brief skips the archive and silently destroys the superseded version; to READ briefs use `recall` with scope 'briefs'.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "worker": {
                    "type": "string",
                    "description": "Worker name — becomes briefs/<worker>.md (server validates it as a single safe path segment: no '/', no '..', no absolute path)"
                },
                "brief": {
                    "type": "string",
                    "description": "One-page kickoff brief: state, today's order of work, decisions made, standing gotchas (template: `templates/kickoff-brief.md` at the reference-deployment root)"
                }
            },
            "required": ["worker", "brief"],
            "additionalProperties": false
        }),
    }]
}

/// The dispatch table entry: `tools/call` for `write_brief` routes
/// here. The handler names the server's OWN process-local session
/// (spec §2: one server process per top-level client session); the
/// unit-level [`write_brief`] takes the session explicitly so the
/// gate tests can drive named sessions.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert("write_brief".to_string(), write_brief_handler);
    handlers
}

fn write_brief_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    // The dispatch names the server's own process-local session
    // (spec §2); clone first so the `&mut` and `&` borrows never
    // overlap inside the call.
    let session = server.session_id.clone();
    write_brief(server, &session, arguments)
}

/// Writes `briefs/<worker>.md` (the contract entry point —
/// `session` is the caller's session identity, the key the uniform
/// gate consults). Pinned order — every refusal leaves ZERO bytes on
/// disk: (1) handshake gate, (2) argument validation, (3) worker
/// safety, (4) today's claim, (5) only then the disk work.
pub fn write_brief(server: &mut Server, session: &str, args: &Value) -> HandlerResult {
    // 1. The uniform gate (spec §4). A pre-handshake refusal is
    //    counted and touches no disk at all.
    if let Err(msg) = gate::gate(server, session) {
        return HandlerResult::Err(msg);
    }

    // 2. Schema validation (zero side effects).
    let (worker, brief) = match parse_brief_args(args) {
        Ok(v) => v,
        Err(msg) => return HandlerResult::Err(msg),
    };

    // 3. Worker safety — BEFORE the briefs/ bucket is ever created,
    //    so a refusal writes nothing.
    if !is_safe_worker(&worker) {
        return HandlerResult::Err(format!(
            "write_brief refused: worker '{worker}' is not a single safe path segment (no '/', no '..', no absolute path) — nothing written"
        ));
    }

    let root = server.root.clone();
    let date = match local_date() {
        Ok(d) => d,
        Err(msg) => return HandlerResult::Err(msg),
    };

    // 4. The claim gate (spec §2/§5), decided BEFORE any brief-bucket
    //    work — a refusal here creates no briefs/ entry. BOTH legs
    //    must pass: the claim file's token for today (re-read from
    //    disk — never a memory map) AND the server's per-session
    //    claim_token for this caller (set by the caller's own
    //    claim_orchestrator — a claim file planted on disk alone is
    //    not a claim).
    let claim_path = root.join(".claims").join(format!("orchestrator-{date}"));
    let file_token = match read_claim(&claim_path) {
        ClaimRead::Held { token, .. } => Some(token),
        ClaimRead::Absent | ClaimRead::Orphan => None,
    };
    let per_session = server.claim_tokens.get(session).cloned();
    let valid = matches!(
        (file_token.as_deref(), per_session.as_deref()),
        (Some(a), Some(b)) if a == b
    );
    if !valid {
        return HandlerResult::Err(format!(
            "write_brief refused: no valid orchestrator_{date} claim — this session holds no claim token for the local date (the on-disk claim record's token and the session's stored token must both agree; a claim file planted on disk alone is not a claim) — nothing written"
        ));
    }

    // 5. The disk work.
    match perform_write_brief(&root, &worker, &brief) {
        Ok(mut v) => {
            // F1 (signoffs/review_Wave4OKF.md 2d): the brief HAS
            // landed, so a missing audit tail is reported as an
            // `audit_error` field on the SUCCESS result — never as
            // `isError`, which spec §2 scopes to refusal/validation/
            // gate-denial and which, rendered over landed bytes, is the
            // both-halves FAIL spec §8 pins. When the audit landed the
            // success shape is byte-identical: no `audit_error` key.
            let audit_error =
                crate::tools::ticks::audit_write(&root, &server.session_id, "write_brief").err();
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
/// exactly the two schema fields, both strings. Refusals name the
/// tool and leave zero side effects.
fn parse_brief_args(args: &Value) -> Result<(String, String), String> {
    let obj = match args.as_object() {
        Some(obj) => obj,
        None => {
            return Err(format!(
                "write_brief refused: arguments must be a JSON object (got {})",
                value_kind(args)
            ))
        }
    };
    let allowed = ["worker", "brief"];
    let unknown: Vec<&String> = obj
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "write_brief refused: unknown field(s) {unknown:?} — the schema allows only: worker, brief"
        ));
    }
    let worker = require_string(obj, "worker")?;
    let brief = require_string(obj, "brief")?;
    Ok((worker, brief))
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
        None => Err(format!(
            "write_brief refused: missing required field '{key}'"
        )),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "write_brief refused: field '{key}' must be a string (got {})",
            value_kind(other)
        )),
    }
}

/// `worker` must be a single safe path segment (spec §3): non-empty,
/// no path separator, no parent-directory component. The check runs
/// before the briefs/ bucket is created, so a refusal writes nothing.
fn is_safe_worker(worker: &str) -> bool {
    !worker.is_empty() && !worker.contains('/') && !worker.contains("..")
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

/// The disk work, reached only after the gate, the argument
/// validation, the worker-safety check and the claim gate have all
/// passed: archive the superseded brief (rename — the old bytes MOVE
/// to the archive, they are never copied-then-deleted), then write
/// the new one atomically (tmp file + rename in the same directory).
fn perform_write_brief(root: &Path, worker: &str, brief: &str) -> Result<Value, String> {
    let briefs_dir = root.join("briefs");
    let target = briefs_dir.join(format!("{worker}.md"));
    let mut archived_to: Option<String> = None;

    if target.exists() {
        // A previous brief exists: it is superseded NOW. The archive
        // name is stamped with the superseded content's own mtime
        // rendered in UTC (contract 44-46; spec §3 verbatim) — the
        // age of the content at the moment it is superseded. Two
        // same-day supersessions can only collide when the superseded
        // files share an mtime second, and in that case the
        // no-overwrite invariant wins: refuse, nothing written.
        let old = std::fs::read(&target)
            .map_err(|e| format!("write_brief failed: reading the superseded briefs/{worker}.md: {e}"))?;
        let mtime = file_mtime_epoch(&target).ok_or_else(|| {
            format!("write_brief refused: cannot read the mtime of the superseded briefs/{worker}.md — no archive stamp can be rendered — nothing written")
        })?;
        let iso = gate::utc_iso8601(mtime);
        let archive_name = format!(
            "{worker}-{}T{}Z.md",
            &iso[..10],
            iso[11..19].replace(':', "")
        );
        let archive_path = briefs_dir.join(&archive_name);
        if archive_path.exists() {
            let existing = std::fs::read(&archive_path).map_err(|e| {
                format!("write_brief failed: reading the existing archive briefs/{archive_name}: {e}")
            })?;
            if existing != old {
                return Err(format!(
                    "write_brief refused: archive briefs/{archive_name} already exists with different content (two supersessions share one mtime second) — no archive is ever overwritten — nothing written"
                ));
            }
            // Byte-identical: exactly this supersession was already
            // archived — idempotent; keep the existing archive and
            // re-report it.
            archived_to = Some(format!("briefs/{archive_name}"));
        } else {
            std::fs::rename(&target, &archive_path).map_err(|e| {
                format!("write_brief failed: archiving the superseded brief to briefs/{archive_name}: {e}")
            })?;
            archived_to = Some(format!("briefs/{archive_name}"));
        }
    } else {
        // The first write for this worker supersedes nothing: no
        // archive. The bucket is created only now — a refused call
        // never creates it.
        std::fs::create_dir_all(&briefs_dir)
            .map_err(|e| format!("write_brief failed: creating the briefs/ bucket: {e}"))?;
    }

    write_atomic(&briefs_dir, worker, brief)?;
    Ok(json!({
        "path": format!("briefs/{worker}.md"),
        "archived_to": archived_to,
    }))
}

/// The mtime of a file as epoch seconds. `None` when the metadata
/// cannot be read or the mtime predates the Unix epoch — the caller
/// then refuses loudly rather than stamping an unknown age.
fn file_mtime_epoch(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// The new brief, written atomically: a tmp file in the target's own
/// directory (same filesystem — the rename is atomic), then renamed
/// over the target. A failed write or rename removes the tmp; the
/// existing target is never left half-written.
fn write_atomic(briefs_dir: &Path, worker: &str, brief: &str) -> Result<(), String> {
    let target_name = format!("{worker}.md");
    let target = briefs_dir.join(&target_name);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(".{worker}.md.tmp-{}-{seq}", std::process::id());
    let tmp = briefs_dir.join(&tmp_name);
    if let Err(e) = std::fs::write(&tmp, brief.as_bytes()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "write_brief failed: writing the new brief (tmp {tmp_name}): {e}"
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "write_brief failed: replacing briefs/{target_name}: {e}"
        ));
    }
    Ok(())
}

/// Monotonic suffix for tmp names (pid + sequence) — no two writers
/// in the same process can collide on a tmp name, and a crashed
/// writer's tmp is identifiable and cleanable.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
