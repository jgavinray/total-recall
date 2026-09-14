//! The `read_signoff` tool (spec §3) — the mandatory first call of
//! every session, and the only path that grants the handshake
//! (spec §4): the session is marked handshaken and persisted in
//! `.state/sessions.json` via `crate::gate::handshake` when — and
//! only when — this call returns successfully.
//!
//! The result carries the signoff.md path, the file's mtime as
//! `as_of` (the age of the data — the same mtime convention
//! `recall` uses for its excerpts), the warm-start block, and the
//! worker signoffs:
//!
//! ```json
//! {
//!   "path": "<configured root>/signoff.md",
//!   "as_of": "2026-09-13T08:14:02Z",
//!   "warm_start": {"header": "…", "ranked": [{"rank": 1, "text": "…"}], "history": ["…"]},
//!   "worker_signoffs": [{"worker": "…", "done": "…", …}]
//! }
//! ```
//!
//! Pinned parsing semantics (wave-2 contract):
//! - the warm-start block is the text between the line
//!   `## If you read nothing else` and the next line starting `## `
//!   (or EOF); `warm_start.header` is the verbatim trimmed text of
//!   the `Last updated:` line immediately preceding the heading —
//!   the nearest preceding NON-BLANK line, when that line is a
//!   `Last updated:` line (the real file separates the two with a
//!   blank line), else `""`;
//! - `ranked` = `{rank, text}` pairs parsed from the numbered lines
//!   `N. …` inside the block (rank = N; text = the remainder after
//!   `N. `, trimmed). Indented continuation lines and any other
//!   block prose are not ranked items.
//! - `history` = the lines under the `## History` heading (to the
//!   next `## ` heading or EOF), trimmed of a leading `- `, as
//!   strings; blank lines are skipped.
//! - `worker_signoffs` = one entry per line starting
//!   `Worker signoff (`: the role runs to the closing `)`; the rest
//!   of the line is ` | `-separated `key: value` fields (the on-disk
//!   separator — a value may itself contain colons, so each fragment
//!   splits at the FIRST `: `). `done`, `unpushed`, `awaits human`,
//!   `still running` are required; `kaibo review`, `workflow`, `ts`,
//!   `session` are optional — an absent optional field is omitted
//!   from the entry. A malformed line — no closing `)`, an empty
//!   role, a fragment that is not `key: value`, or a missing
//!   required field — is tolerated by SKIPPING the line; the read
//!   still succeeds (spec §1: parsing is tolerant of shape, the
//!   server never crashes on file content).
//!
//! A readable signoff.md always yields a successful read: missing
//! sections (no heading, no ranked lines, no History, no signoffs)
//! parse to empty. A missing or unreadable file is an I/O failure —
//! a loud `Err` and NO handshake (spec §3: the handshake is recorded
//! when the call "returns successfully"), so the gate stays closed
//! until the file is readable.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::gate;
use crate::rpc::{self, HandlerResult, Server, Tool};

/// The `read_signoff` tool shape advertised by `tools/list` — the
/// description carries the MANDATORY contract text (spec §4), and the
/// input schema is the pinned empty-object shape (spec §3).
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "read_signoff".to_string(),
        description: "MANDATORY FIRST CALL of every session. Returns the signoff.md warm-start block, the ranked 'if you read nothing else' list, and worker signoffs. RECORDS the session handshake: no mutating tool is accepted until this call returns successfully. Contract: first action of every session; no work before it returns.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    }]
}

/// The dispatch table entry: `tools/call` for `read_signoff` routes
/// here. The handler names the server's OWN process-local session
/// (spec §2: one server process per top-level client session); the
/// unit-level [`read_signoff`] takes the session explicitly so the
/// gate tests can drive named sessions.
pub fn handlers() -> HashMap<String, rpc::Handler> {
    let mut handlers: HashMap<String, rpc::Handler> = HashMap::new();
    handlers.insert("read_signoff".to_string(), read_signoff_handler);
    handlers
}

fn read_signoff_handler(server: &mut Server, _arguments: &Value) -> HandlerResult {
    // The dispatch names the server's own process-local session
    // (spec §2); clone first so the `&mut` and `&` borrows never
    // overlap inside the call.
    let session = server.session_id.clone();
    read_signoff(server, &session)
}

/// The mandatory first call of every session (spec §3). Reads
/// signoff.md, parses the pinned shapes (module docs), and — on
/// success — grants the session the handshake (marked in the
/// process-local map AND persisted in `.state/sessions.json`). A
/// missing or unreadable file is an `Err` with NO handshake granted.
pub fn read_signoff(server: &mut Server, session: &str) -> HandlerResult {
    let path = server.root.join("signoff.md");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return HandlerResult::Err(format!(
                "read_signoff failed: cannot read {path:?}: {err} — the session is NOT handshaken; no mutating tool is accepted until this call returns successfully"
            ));
        }
    };
    let as_of = match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
        Ok(mtime) => {
            let secs = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            gate::utc_iso8601(secs)
        }
        Err(err) => {
            return HandlerResult::Err(format!(
                "read_signoff failed: cannot stat {path:?}: {err} — the session is NOT handshaken; no mutating tool is accepted until this call returns successfully"
            ));
        }
    };
    let parsed = parse_signoff(&text);
    if !gate::handshake(server, session) {
        return HandlerResult::Err(format!(
            "read_signoff failed: the handshake record could not be persisted to .state/sessions.json (I/O error — see the server log) — the session is NOT handshaken; no mutating tool is accepted until this call returns successfully"
        ));
    }
    HandlerResult::Ok(json!({
        "path": path.display().to_string(),
        "as_of": as_of,
        "warm_start": json!({
            "header": parsed.header,
            "ranked": parsed.ranked,
            "history": parsed.history
        }),
        "worker_signoffs": parsed.worker_signoffs
    }))
}

struct ParsedSignoff {
    header: String,
    ranked: Vec<Value>,
    history: Vec<String>,
    worker_signoffs: Vec<Value>,
}

fn parse_signoff(text: &str) -> ParsedSignoff {
    let lines: Vec<&str> = text.lines().collect();
    let warm = parse_warm_start(&lines);
    let history = parse_history(&lines);
    let worker_signoffs = lines
        .iter()
        .filter(|line| line.starts_with("Worker signoff ("))
        .filter_map(|line| parse_worker_line(line))
        .collect();
    ParsedSignoff {
        header: warm.header,
        ranked: warm.ranked,
        history,
        worker_signoffs,
    }
}

struct ParsedWarmStart {
    header: String,
    ranked: Vec<Value>,
}

/// The warm-start block: between the line `## If you read nothing
/// else` and the next line starting `## ` (or EOF). The header is
/// the nearest preceding non-blank line, verbatim and trimmed, when
/// that line is a `Last updated:` line, else `""` (wave-2 contract —
/// "immediately preceding" skips blank lines: the real file separates
/// the two with one).
fn parse_warm_start(lines: &[&str]) -> ParsedWarmStart {
    let Some(heading) = lines
        .iter()
        .position(|line| *line == "## If you read nothing else")
    else {
        return ParsedWarmStart {
            header: String::new(),
            ranked: Vec::new(),
        };
    };
    let end = lines[heading + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map(|offset| heading + 1 + offset)
        .unwrap_or(lines.len());

    let header = lines[..heading]
        .iter()
        .rev()
        .find(|line| !line.trim().is_empty())
        .filter(|line| line.starts_with("Last updated:"))
        .map(|line| line.trim().to_string())
        .unwrap_or_default();

    let ranked = lines[heading + 1..end]
        .iter()
        .filter_map(|line| parse_ranked_line(line))
        .map(|(rank, text)| json!({"rank": rank, "text": text}))
        .collect();

    ParsedWarmStart { header, ranked }
}

/// The lines under the `## History` heading (to the next `## `
/// heading or EOF), trimmed of a leading `- ` (blank lines skipped)
/// — the superseded warm-start blocks, newest last (wave-2 contract).
fn parse_history(lines: &[&str]) -> Vec<String> {
    let Some(heading) = lines.iter().position(|line| *line == "## History") else {
        return Vec::new();
    };
    let end = lines[heading + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map(|offset| heading + 1 + offset)
        .unwrap_or(lines.len());
    lines[heading + 1..end]
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.strip_prefix("- ")
                .map(|rest| rest.trim().to_string())
                .unwrap_or_else(|| line.trim().to_string())
        })
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// A numbered line `N. text` inside the warm-start block: rank = N,
/// text = the remainder after `N. `, trimmed (wave-2 contract). A
/// line that does not start with digits followed by `". "` (indented
/// continuation prose, headings, bare prose) is not a ranked item.
fn parse_ranked_line(line: &str) -> Option<(u64, String)> {
    let mut rest = line;
    let mut rank: u64 = 0;
    let mut has_digit = false;
    while let Some(ch) = rest.chars().next() {
        if ch.is_ascii_digit() {
            rank = rank.saturating_mul(10).saturating_add((ch as u64) - ('0' as u64));
            has_digit = true;
            rest = &rest[ch.len_utf8()..];
        } else {
            break;
        }
    }
    if !has_digit {
        return None;
    }
    let dot = rest.chars().next()?;
    if dot != '.' {
        return None;
    }
    rest = &rest[dot.len_utf8()..];
    let space = rest.chars().next()?;
    if !space.is_whitespace() {
        return None; // "1.x" / bare "1." — not the `N. ` numbered form
    }
    Some((rank, rest[space.len_utf8()..].trim().to_string()))
}

/// One worker-signoff line (wave-2 contract): the line starts
/// `Worker signoff (`; the role runs to the closing `)`; the rest of
/// the line is ` | `-separated `key: value` fields — `done`,
/// `unpushed`, `awaits human`, `still running` required; `kaibo
/// review`, `workflow`, `ts`, `session` optional (absent fields are
/// omitted from the entry). Each fragment splits at the FIRST `: `,
/// so a value may contain colons; unknown keys are ignored
/// (forward-compatible). A line that is malformed in any of the
/// pinned ways is tolerated by SKIPPING, never an error.
fn parse_worker_line(line: &str) -> Option<Value> {
    let rest = line.strip_prefix("Worker signoff (")?;
    let close = rest.find(')')?;
    let worker = rest[..close].trim();
    if worker.is_empty() {
        return None;
    }
    let after = &rest[close + 1..];
    if after.is_empty() || !after.starts_with(" | ") {
        return None;
    }
    let mut done: Option<&str> = None;
    let mut unpushed: Option<&str> = None;
    let mut awaits_human: Option<&str> = None;
    let mut still_running: Option<&str> = None;
    let mut kaibo_review: Option<&str> = None;
    let mut workflow: Option<&str> = None;
    let mut ts: Option<&str> = None;
    let mut session: Option<&str> = None;
    for fragment in after[3..].split(" | ") {
        // A fragment that is not `key: value` is malformed: the whole
        // line is skipped (tolerant parsing, spec §1).
        let (key, value) = fragment.split_once(": ")?;
        match key.trim() {
            "done" => done = Some(value.trim()),
            "unpushed" => unpushed = Some(value.trim()),
            "awaits human" => awaits_human = Some(value.trim()),
            "still running" => still_running = Some(value.trim()),
            "kaibo review" => kaibo_review = Some(value.trim()),
            "workflow" => workflow = Some(value.trim()),
            "ts" => ts = Some(value.trim()),
            "session" => session = Some(value.trim()),
            _ => {} // a field this parser does not know: ignored
        }
    }
    let (done, unpushed, awaits_human, still_running) =
        match (done, unpushed, awaits_human, still_running) {
            (Some(done), Some(unpushed), Some(awaits_human), Some(still_running)) => {
                (done, unpushed, awaits_human, still_running)
            }
            _ => return None, // malformed: a required field is missing
        };
    let mut entry = json!({
        "worker": worker,
        "done": done,
        "unpushed": unpushed,
        "awaits_human": awaits_human,
        "still_running": still_running
    });
    if let Some(v) = kaibo_review {
        entry["kaibo_review"] = json!(v);
    }
    if let Some(v) = workflow {
        entry["workflow"] = json!(v);
    }
    if let Some(v) = ts {
        entry["ts"] = json!(v);
    }
    if let Some(v) = session {
        entry["session"] = json!(v);
    }
    Some(entry)
}
