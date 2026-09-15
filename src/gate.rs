//! Handshake enforcement (spec §4) — the uniform server-side gate.
//!
//! The gate is uniform over every disk-touching call: all five append
//! paths (`append_signoff`, `write_dayfile`, `write_brief`,
//! `write_warm_start`, `log_tick`) and `claim_orchestrator` refuse
//! until `read_signoff` has succeeded this session. There is no
//! audit-side exemption — `log_tick` appends to `.audit/` and is
//! gated exactly like the rest. Only the read/audit-side tools
//! (`read_signoff` itself, `recall`, `last_tick`, `session_compliance`)
//! run ungated (spec §4).
//!
//! Pinned semantics (wave-2 contract):
//! - `gate` refuses EVERY gated tool until `read_signoff` succeeded
//!   this session. The refusal message is exactly
//!   `handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).`,
//!   restating the rule at the failure moment (spec §4) — emitted identically
//!   by EVERY gated tool (naive-client round: the private short variant in
//!   signoff_append was cut over to this shared constant; the FOUND-1 tail
//!   closes the contradiction with read_signoff's own cold-start error,
//!   which says retrying cannot fix a missing signoff.md).
//! - A refused attempt BEFORE the handshake increments
//!   `server.attempted_write_before_handshake[session]` —
//!   server-observable, so `session_compliance` can report it
//!   (spec §3; §8 test 12). The write itself never lands.
//! - `handshake` marks `handshaken[session] = true` and persists the
//!   session id in `.state/sessions.json` (an array of
//!   `{session_id, handshaked_at}`).
//!
//! `handshaken` is PROCESS-LOCAL by construction (spec §4: restart =>
//! re-handshake); `.state/sessions.json` is the on-disk audit trace of
//! which sessions handshaken on this root. Within one process the
//! record is exact — deduplicated per session id, idempotent on
//! re-handshake — and the write is a per-call tmp file renamed into
//! place, so a crash can never leave the file half-written. Two
//! concurrent server processes on one root may race the record (a
//! lost entry, never corruption): the authoritative gate state is the
//! in-process map, and the spec §2 lock-file protocol covers the
//! append paths, not this audit trace.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use crate::rpc::Server;


/// The full refusal message a gated tool reports: the pinned text,
/// restating the rule at the failure moment (spec §4), plus the
/// FOUND-1 cold-start clause — when `read_signoff` itself reports the
/// root unprovisioned, the refusal must not instruct a blind retry:
/// that failure is provisioning, not call order.
pub const HANDSHAKE_REFUSAL_MESSAGE: &str =
    "handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).";

/// Uniform gate (spec §4): refuse EVERY disk-touching call for a
/// session that has not yet handshaken.
///
/// On refusal the attempt is counted in
/// `server.attempted_write_before_handshake[session]` ("observable at
/// the gate") and the pinned refusal message is returned as the
/// handler's error — which the protocol layer renders as a normal
/// result carrying `isError: true` plus a `content` array, never a
/// JSON-RPC `error` object (spec §2).
pub fn gate(server: &mut Server, session: &str) -> Result<(), String> {
    if server.handshaken.get(session).copied().unwrap_or(false) {
        return Ok(());
    }
    *server
        .attempted_write_before_handshake
        .entry(session.to_string())
        .or_insert(0) += 1;
    Err(HANDSHAKE_REFUSAL_MESSAGE.to_string())
}

/// Grant the handshake to `session`: mark `handshaken[session] = true`
/// and persist the session id in `.state/sessions.json` (an array of
/// `{session_id, handshaked_at}` — wave-2 contract).
///
/// Returns `true` iff the session is handshaken after the call: it
/// already was (idempotent no-op — no record rewrite), or this call
/// marked it AND the record is on disk. A persistence failure is
/// LOUD: the mark is rolled back (the gate stays closed) and `false`
/// is returned, so a caller that grants work on the return value
/// refuses the session until the record is actually persisted.
pub fn handshake(server: &mut Server, session: &str) -> bool {
    if server.handshaken.get(session).copied().unwrap_or(false) {
        return true;
    }
    server.handshaken.insert(session.to_string(), true);
    if let Err(err) = persist_session_record(server.root.as_path(), session) {
        // Roll back: a handshake that is not on disk did not happen.
        server.handshaken.remove(session);
        eprintln!("[gate] handshake record not persisted: {err}");
        return false;
    }
    true
}

/// Per-call uniqueness for the tmp file name: several handshakes can
/// run in the same process (tests race many servers in threads), and
/// two of them must never share one tmp path — the rename half of
/// the atomic write would race on it. The pid keeps the name unique
/// across processes on the same root.
static SESSIONS_TMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The `.state/sessions.json` writer: read-modify-write — the record
/// is re-read on every handshake, deduplicated per session id (a
/// session handshakes once; a re-handshake is a no-op that rewrites
/// nothing), then written as a per-call tmp file renamed into place
/// (the atomic rename means the file is never observed half-written).
fn persist_session_record(root: &Path, session: &str) -> Result<(), String> {
    let state_dir = root.join(".state");
    std::fs::create_dir_all(&state_dir).map_err(|err| format!("create .state/: {err}"))?;
    let path = state_dir.join("sessions.json");

    let mut entries: Vec<Value> = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text)
                .map_err(|err| format!("parse .state/sessions.json: {err}"))?;
            value.as_array().cloned().ok_or_else(|| {
                "refusing to append to a malformed .state/sessions.json (not a JSON array)".to_string()
            })?
        }
        Err(_) => Vec::new(), // absent: the first record on this root
    };

    if entries
        .iter()
        .any(|entry| entry.get("session_id").and_then(Value::as_str) == Some(session))
    {
        return Ok(()); // already recorded — idempotent, no rewrite
    }

    entries.push(json!({
        "session_id": session,
        "handshaked_at": now_utc_iso8601()
    }));

    let tmp = state_dir.join(format!(
        ".sessions.json.tmp-{}-{}",
        std::process::id(),
        SESSIONS_TMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let serialized = serde_json::to_string(&entries)
        .map_err(|err| format!("serialize .state/sessions.json: {err}"))?;
    if let Err(err) = std::fs::write(&tmp, serialized) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write .state/sessions.json: {err}"));
    }
    if let Err(err) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename .state/sessions.json into place: {err}"));
    }
    Ok(())
}

/// Render seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ` —
/// all emitted timestamps are UTC with a trailing `Z` (spec §2).
///
/// `pub(crate)`: `tools::signoff_read` stamps its `as_of` field with
/// the signoff.md mtime through this same std-only renderer (the
/// crate ships std + serde_json only — no date dependency).
pub(crate) fn utc_iso8601(secs: i64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, rem / 60 % 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        month, day, hour, minute, second
    )
}

/// Current UTC wall-clock (the `handshaked_at` stamp), per
/// [`utc_iso8601`].
fn now_utc_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    utc_iso8601(secs)
}

/// Howard Hinnant's `civil_from_days`: pure-integer calendar
/// arithmetic for the UTC date (no third-party date dependency, per
/// §2). Days are counted from the Unix epoch; internally they are
/// re-based to the March-anchored proleptic Gregorian calendar
/// (719468 days between 0000-03-01 and 1970-01-01) and divided into
/// 400-year eras (146097 days — the calendar's exact repetition
/// period), so the century leap rules fall out of the integer math.
/// (Same arithmetic as `config::civil_from_days`.)
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
