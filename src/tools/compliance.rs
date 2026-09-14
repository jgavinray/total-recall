//! The `session_compliance` tool (spec §3) — the admin/debug surface
//! that makes the memory protocol's enforcement observable, and one of
//! the four UNGATED tools (spec §4).
//!
//! Everything here is DERIVED, nothing is remembered:
//! - `write_count` and `first_write_ts` come from the audit trail
//!   (`.audit/*.jsonl`), the entries the mutating tools emit through
//!   `tools::ticks::audit_write`;
//! - `handshaked_at` comes from `.state/sessions.json`, the record
//!   `gate::handshake` persists — so a report can be checked against the
//!   disk that granted the session, not against this process's memory;
//! - `attempted_write_before_handshake` and `refused_count` come from
//!   the CURRENT process's gate counter (`server
//!   .attempted_write_before_handshake[session]`), which is the only
//!   place refusals exist at all: a refused call writes nothing, so
//!   there is no `wrote_without_handshake` metric that could ever exist
//!   (spec §3), and the counter is the honest half of the story —
//!   observable at the gate.
//!
//! Identity, stated plainly because two names are in play: the report
//! row is identified by the process-local `session_id` the mutating
//! tools stamped into the audit log (spec §2 — one server process per
//! top-level client session, so this process's writes and this process's
//! gate counters are the same session's), while the CALLER's `session`
//! argument is the key the gate and the handshake record are kept under.
//! In the live server they are the same string; at unit level the tests
//! drive named sessions, and the row still reports the truth of both
//! sources rather than picking one and lying about the other.
//!
//! The caller's row is first; sessions seen in the audit trail that this
//! process was not asked about follow it in id order (the report is
//! audit-derived, so a shared root's other sessions are visible, with
//! their on-disk handshake record and whatever this process happens to
//! have counted for them — no counter, no accusation).

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::rpc::{Handler, HandlerResult, Server, Tool};

/// The tool shape advertised by `tools/list`: the spec §3 description
/// verbatim, and its pinned empty-object schema (no parameters — the
/// report is about what the server has measured, not what a caller asks
/// for).
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "session_compliance".to_string(),
        description: "Reach for this when you must VERIFY a session actually followed the memory protocol (admin/debug) — UNGATED and always available. Audit-derived report: per session — handshake time, first-write time, attempted_write_before_handshake (true when a call to a gated tool was ATTEMPTED and refused before the handshake — observable at the gate; the write itself never lands, so no 'wrote_without_handshake' metric can ever exist), write count, refused count. The numbers come from the on-disk audit trail, the handshake record and this process's gate counters — never from what a client claims. Compliance is a measured number, not a vibe.".to_string(),
        input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
    }]
}

/// The dispatch entry this module owns: the `tools/call` entry point for
/// `session_compliance`. The adapter names the server's own
/// process-local session (spec §2); the contract entry point takes the
/// session explicitly so the gate tests can drive named sessions.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert("session_compliance".to_string(), session_compliance_handler);
    handlers
}

/// The `tools/call` dispatch entry (see [`handlers`]).
fn session_compliance_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    session_compliance(server, &session, arguments)
}

/// Build the report (the contract entry point).
pub fn session_compliance(server: &mut Server, session: &str, arguments: &Value) -> HandlerResult {
    // The schema admits no fields; an argument that isn't `{}` is a
    // client misunderstanding, and silence about it would let the
    // caller believe a filter they never got had been applied.
    match arguments {
        Value::Object(map) if map.is_empty() => {}
        Value::Object(map) => {
            let fields: Vec<&str> = map.keys().map(String::as_str).collect();
            return HandlerResult::Err(format!(
                "session_compliance refused: unknown field(s) {} — the schema takes no arguments",
                fields
                    .iter()
                    .map(|f| format!("'{f}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        other => {
            return HandlerResult::Err(format!(
                "session_compliance refused: arguments must be a JSON object (got {other})"
            ))
        }
    }

    let root = server.root.as_path();
    let audit = match audit_entries(root) {
        Ok(entries) => entries,
        Err(err) => return HandlerResult::Err(err),
    };
    let records = match handshake_records(root) {
        Ok(records) => records,
        Err(err) => return HandlerResult::Err(err),
    };

    // The reported identity is the process-local session id the mutating
    // tools stamped into the audit trail (spec §2: one server process per
    // top-level client session, so the writes, the gate counters and the
    // handshake record all belong to the same session). The gate counter
    // and the handshake record are keyed by the session the CALLER named —
    // for the live server that is the same string; at unit level it is the
    // test's label. Each source is consulted under its own key rather than
    // one being renamed into the other, which is what keeps a count from
    // being attributed to an identity that never performed it.
    let identity = server.session_id.clone();
    let mut rows: Vec<(String, String)> = vec![(identity.clone(), session.to_string())];
    for entry in &audit {
        if let Some(id) = entry.get("session_id").and_then(Value::as_str) {
            if id != identity && !rows.iter().any(|(known, _)| known == id) {
                rows.push((id.to_string(), id.to_string()));
            }
        }
    }
    let tail = &mut rows[1..];
    tail.sort_by(|a, b| a.0.cmp(&b.0));

    let sessions: Vec<Value> = rows
        .iter()
        .map(|(id, gate_key)| {
            let writes: Vec<&Value> = audit
                .iter()
                .filter(|entry| entry.get("session_id").and_then(Value::as_str) == Some(id.as_str()))
                .collect();
            // The trail is append-ordered per file and the files are read
            // in date order, so the FIRST entry carrying the identity is
            // its first write; no clock is consulted — that is what makes
            // the number audit-derived rather than a process-lifetime
            // guess.
            let first_write_ts = writes
                .first()
                .and_then(|entry| entry.get("ts"))
                .and_then(Value::as_str)
                .map(Value::from)
                .unwrap_or(Value::Null);
            let refused_count = server
                .attempted_write_before_handshake
                .get(gate_key)
                .copied()
                .unwrap_or(0);
            json!({
                "session_id": id,
                "handshaked_at": records.get(gate_key).cloned().unwrap_or(Value::Null),
                "first_write_ts": first_write_ts,
                // A refusal counted under some other key is not this
                // session's record.
                "attempted_write_before_handshake": refused_count > 0,
                "write_count": writes.len(),
                "refused_count": refused_count,
            })
        })
        .collect();

    HandlerResult::Ok(json!({"sessions": sessions}))
}

// ---------------------------------------------------------------------------
// The two on-disk sources
// ---------------------------------------------------------------------------

/// Every audit entry in `.audit/*.jsonl`, files read in date order and
/// lines in file order (the append order). A missing `.audit/` is an
/// empty trail, not a failure — a session that has done nothing is
/// compliant, not broken. A file that cannot be READ is LOUD: counts
/// that cannot be counted must not be reported as zero
/// ("a check that can't run must fail loudly"). A malformed LINE is
/// skipped, never fatal (spec §1's tolerance for what it cannot parse —
/// it is never rewritten from here, this tool has no write path).
fn audit_entries(root: &std::path::Path) -> Result<Vec<Value>, String> {
    let dir = root.join(".audit");
    let mut out: Vec<Value> = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|err| format!("session_compliance refused: cannot list .audit/: {err}"))?
    {
        let entry = entry.map_err(|err| {
            format!("session_compliance refused: listing .audit/: {err}")
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") || name.starts_with('.') {
            continue;
        }
        if !matches!(entry.file_type(), Ok(ft) if ft.is_file()) {
            continue;
        }
        files.push((name, entry.path()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path) in files {
        let text = std::fs::read_to_string(&path).map_err(|err| {
            format!(
                "session_compliance refused: cannot read the audit trail {path:?}: {err} — write counts come from it, and a trail that cannot be read is not evidence of zero writes"
            )
        })?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                if value.is_object() {
                    out.push(value);
                }
            }
        }
    }
    Ok(out)
}

/// `session_id -> handshaked_at` from `.state/sessions.json` (the array
/// of `{session_id, handshaked_at}` `gate::handshake` persists). Absent
/// or unreadable means no session has a persisted handshake record —
/// which is itself the finding, so it reports nulls rather than
/// refusing; the file is written atomically by the gate, so a partial
/// read is not a state this tool has to guess at.
fn handshake_records(root: &std::path::Path) -> Result<HashMap<String, Value>, String> {
    let path = root.join(".state").join("sessions.json");
    let mut out: HashMap<String, Value> = HashMap::new();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => {
            return Err(format!(
                "session_compliance refused: cannot read the handshake record {path:?}: {err} — handshake times come from it"
            ))
        }
    };
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        format!("session_compliance refused: {path:?} is not readable JSON: {err}")
    })?;
    let entries = match value.as_array() {
        Some(entries) => entries,
        None => {
            return Err(format!(
                "session_compliance refused: {path:?} is not the array of handshake records the gate writes"
            ))
        }
    };
    for entry in entries {
        if let (Some(id), Some(at)) = (
            entry.get("session_id").and_then(Value::as_str),
            entry.get("handshaked_at"),
        ) {
            out.insert(id.to_string(), at.clone());
        }
    }
    Ok(out)
}
