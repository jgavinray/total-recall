//! The tick tools (spec §3 `log_tick` / `last_tick`, §7's 45-minute tick
//! window, §8 test 11).
//!
//! `log_tick` is the sixth append path and is therefore GATED exactly
//! like the other five (spec §4: "There is no audit-side exemption —
//! `log_tick` appends to `.audit/` and is therefore gated exactly like
//! the rest"). `last_tick` is a pure read and runs UNGATED.
//!
//! Pinned semantics:
//! - the audit trail is `.audit/YYYY-MM-DD.jsonl`, the date being the
//!   root's LOCAL date from the recorded timezone fingerprint — the
//!   same authority `write_dayfile` uses (`crate::tools::claims::local_date`,
//!   reused rather than a second copy of the arithmetic);
//! - a tick line is exactly `{ts, session_id, check, result}`, the
//!   `session_id` being the server's own process-local identity (spec §2
//!   — the caller's transport label is not what gets recorded), and the
//!   `ts` UTC ISO-8601Z (spec §7: "every entry carries `ts` (ISO,
//!   UTC)");
//! - the append takes the `.locks/` append mutex for that audit file
//!   (spec §2: one lock name per file), the same bounded-retry /
//!   stale-takeover protocol as signoff, day files, briefs and claims;
//! - a tick that cannot be written is LOUD: the audit tail is the
//!   compliance evidence, and a silent gap in it is the failure mode
//!   §1's "a check that can't run must fail loudly" prohibits. It does
//!   not un-land the caller's own write (which already succeeded), so
//!   the callers decide between failing the call and reporting it;
//! - `last_tick` scans EVERY `.audit/*.jsonl` file (so a check's history
//!   is not hostage to the date boundary), ignores lines it cannot
//!   parse — a corrupt tail line is tolerated, never an error (spec §1:
//!   "a corrupt existing file is refused LOUDLY, never silently
//!   rewritten", and these lines are read-only here) — and distinguishes
//!   the two loud failures the spec pins: an UNREGISTERED check (no
//!   ticks on record at all) and a registered one whose latest tick has
//!   aged TO or PAST the 45-minute window, whose text is pinned verbatim:
//!   `silence is never good news: check X last ticked <ts> (<age> ago)`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::gate;
use crate::rpc::{Handler, HandlerResult, Server, Tool};
use crate::tools::claims;
use crate::tools::index;

/// The tick window (spec §7 server constant, 45 min; §8 test 11 pins
/// the boundary at exactly 45m → error, 44m → ok).
pub const TICK_WINDOW_SECS: i64 = 45 * 60;

/// The audit directory (spec §2). A later wave may rotate it; it is
/// never a search bucket (spec §10) and nothing here reads outside it.
const AUDIT_DIR: &str = ".audit";

/// The tool shapes advertised by `tools/list` — descriptions carry the
/// spec §3 contract text verbatim, schemas are spec §3's pinned shapes.
pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "log_tick".to_string(),
            description: "Records a check result as {ts, session_id, check, result} in .audit/YYYY-MM-DD.jsonl (the only other append path). Requires the handshake like every other append path (§4). Idle ticks are a local stat of shared files; network/MCP calls on demand, not per-heartbeat.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "check": {"type": "string", "description": "Registered check name (stable identifier)"},
                    "result": {"type": "string", "description": "Observed status — report experience, not synthesis"}
                },
                "required": ["check", "result"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "last_tick".to_string(),
            description: "Returns the latest tick for a check. Errors LOUDLY when a registered check (has ticks on record) has aged to or past the tick window (server constant, 45 min; the boundary is pinned by test 11): 'silence is never good news: check X last ticked <ts> (<age> ago)'. Unregistered check is an error too.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "check": {"type": "string", "description": "Registered check name"}
                },
                "required": ["check"],
                "additionalProperties": false
            }),
        },
    ]
}

/// The dispatch entries this module owns. In the live server the session
/// IS the process-local `server.session_id` (spec §2), so the adapters
/// pass it through; the contract entry points take it explicitly so the
/// gate tests can drive named sessions.
pub fn handlers() -> HashMap<String, Handler> {
    let mut handlers: HashMap<String, Handler> = HashMap::new();
    handlers.insert("log_tick".to_string(), log_tick_handler);
    handlers.insert("last_tick".to_string(), last_tick_handler);
    handlers
}

/// The `tools/call` dispatch entry for `log_tick` (see [`handlers`]).
fn log_tick_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    log_tick(server, &session, arguments)
}

/// The `tools/call` dispatch entry for `last_tick` (see [`handlers`]).
fn last_tick_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    last_tick(server, &session, arguments)
}

/// Record one check result (the contract entry point).
///
/// Order of operations, pinned so a refusal leaves ZERO bytes (spec §8:
/// "a server that says `isError` and writes anyway FAILS"):
/// 1. the handshake gate (spec §4) — refused attempts are counted at the
///    gate and NOTHING is opened, created or written;
/// 2. argument validation against the pinned schema;
/// 3. the append itself, under `.locks/audit-<date>.jsonl.lock`.
pub fn log_tick(server: &mut Server, session: &str, arguments: &Value) -> HandlerResult {
    // 1. gated, exactly like the other append paths (spec §4).
    if let Err(refusal) = gate::gate(server, session) {
        return HandlerResult::Err(refusal);
    }

    // 2. arguments — the pinned two-field shape, both required strings.
    let args = match arguments.as_object() {
        Some(map) => map,
        None => {
            return HandlerResult::Err(
                "log_tick refused: arguments must be a JSON object ({\"check\": …, \"result\": …})"
                    .to_string(),
            )
        }
    };
    for key in args.keys() {
        if !matches!(key.as_str(), "check" | "result") {
            return HandlerResult::Err(format!(
                "log_tick refused: unknown field '{key}' — the schema allows only check, result"
            ));
        }
    }
    let (check, result) = match (args.get("check"), args.get("result")) {
        (Some(check), Some(result)) => {
            let check = match check.as_str() {
                Some(text) if !text.trim().is_empty() => text.trim().to_string(),
                Some(_) => {
                    return HandlerResult::Err(
                        "log_tick refused: field 'check' must not be empty — an unnamed check can never be ticked again".to_string(),
                    )
                }
                None => {
                    return HandlerResult::Err(format!(
                        "log_tick refused: field 'check' must be a string (got {check})"
                    ))
                }
            };
            let result = match result.as_str() {
                Some(text) => text.to_string(),
                None => {
                    return HandlerResult::Err(format!(
                        "log_tick refused: field 'result' must be a string (got {result})"
                    ))
                }
            };
            (check, result)
        }
        _ => {
            return HandlerResult::Err(
                "log_tick refused: missing required field 'check' and/or 'result'".to_string(),
            )
        }
    };

    // 3. the append. `ts` is stamped by the SERVER, never accepted from
    // the caller, and the session recorded is the process-local identity
    // (spec §2), not the transport label the caller passed.
    let session_id = server.session_id.clone();
    let root = server.root.as_path();
    let ts = match now_utc() {
        Ok(ts) => ts,
        Err(err) => return HandlerResult::Err(format!("log_tick refused: {err}")),
    };
    let entry = json!({"ts": ts, "session_id": session_id, "check": check, "result": result});
    let path = match append_audit(root, &entry, "log_tick") {
        Ok(path) => path,
        Err(err) => return HandlerResult::Err(err),
    };
    HandlerResult::Ok(json!({
        "logged": true,
        "path": path,
        "ts": ts,
        "check": check,
        "result": result,
    }))
}

/// Report the latest tick for `check` (the contract entry point).
/// UNGATED (spec §4 read/audit side).
///
/// Both silence failures are loud and are distinguished from each other:
/// - the check has no ticks on record at all (UNREGISTERED — nothing has
///   ever reported it, so it is not being monitored);
/// - the check has ticks but the newest has aged to or past the window,
///   which reports the pinned text verbatim.
///
/// A tick whose `ts` cannot be parsed is not treated as fresh: freshness
/// that cannot be computed is not freshness.
pub fn last_tick(server: &mut Server, _session: &str, arguments: &Value) -> HandlerResult {
    let args = match arguments.as_object() {
        Some(map) => map,
        None => {
            return HandlerResult::Err(
                "last_tick refused: arguments must be a JSON object ({\"check\": …})".to_string(),
            )
        }
    };
    for key in args.keys() {
        if key != "check" {
            return HandlerResult::Err(format!(
                "last_tick refused: unknown field '{key}' — the schema allows only check"
            ));
        }
    }
    let check = match args.get("check") {
        Some(value) => match value.as_str() {
            Some(text) if !text.trim().is_empty() => text.trim().to_string(),
            Some(_) => {
                return HandlerResult::Err(
                    "last_tick refused: field 'check' must not be empty".to_string(),
                )
            }
            None => {
                return HandlerResult::Err(format!(
                    "last_tick refused: field 'check' must be a string (got {value})"
                ))
            }
        },
        None => {
            return HandlerResult::Err(
                "last_tick refused: missing required field 'check'".to_string(),
            )
        }
    };

    // Registration is derived from the audit log itself: a registered
    // check is one that has ticks on record (spec §3). There is no
    // registry to drift out of sync with the evidence.
    let ticks = match collect_ticks(server.root.as_path(), &check) {
        Ok(ticks) => ticks,
        Err(err) => return HandlerResult::Err(err),
    };
    if ticks.is_empty() {
        return HandlerResult::Err(format!(
            "last_tick refused: check '{check}' is not registered — no ticks on record in .audit/*.jsonl; a check is registered by log_tick recording it"
        ));
    }
    // Newest wins; `collect_ticks` walks the files in date order, so a
    // later file's tick breaks a tie.
    let (epoch, latest) = match ticks
        .iter()
        .filter_map(|tick| parse_utc_iso8601(tick["ts"].as_str().unwrap_or("")).map(|e| (e, tick)))
        .max_by_key(|(epoch, _)| *epoch)
    {
        Some(found) => found,
        None => {
            // Registered but unreadable: report the condition instead of
            // certifying a tick whose age nobody can compute.
            return HandlerResult::Err(format!(
                "last_tick refused: check '{check}' has {} tick(s) on record but none with a readable ISO-8601Z `ts` — freshness cannot be computed from them",
                ticks.len()
            ));
        }
    };
    let ts = latest["ts"].as_str().unwrap_or("").to_string();
    let now = match now_epoch() {
        Ok(now) => now as i64,
        Err(err) => return HandlerResult::Err(format!("last_tick refused: {err}")),
    };
    let age = now - epoch;
    if age >= TICK_WINDOW_SECS {
        return HandlerResult::Err(format!(
            "silence is never good news: check {check} last ticked {ts} ({})",
            render_age(age)
        ));
    }
    let mut report = json!({"check": check, "ts": ts});
    if let Some(object) = report.as_object_mut() {
        for key in ["session_id", "result"] {
            if let Some(value) = latest.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    HandlerResult::Ok(report)
}

/// Emit one mutating-write audit entry (`{ts, session_id, tool}`) — the
/// hook the append paths call on their success path, so
/// `session_compliance`'s counts are measured rather than remembered
/// (§3: "Compliance is a measured number, not a vibe").
///
/// Loud by contract, and it does not lie in either direction: the
/// caller's write HAS landed, so this reports the missing tail rather
/// than pretending the write failed, and a caller that would rather fail
/// the call than proceed without evidence can propagate the error.
pub fn audit_write(root: &Path, session_id: &str, tool: &str) -> Result<(), String> {
    let ts = now_utc()?;
    let entry = json!({"ts": ts, "session_id": session_id, "tool": tool});
    append_audit(root, &entry, "audit").map(|_| ())
}

// ---------------------------------------------------------------------------
// The audit writer and reader
// ---------------------------------------------------------------------------

/// Append one JSON line to today's audit file under the `.locks/` append
/// mutex for that file (spec §2: one lock name per file). Returns the
/// root-relative path it wrote. Every failure names the path and the
/// reason; nothing half-written is left behind (the append is one
/// `write_all` of a complete line, so a torn line would require the
/// process to die mid-write, which staleness-retry tolerates on read).
fn append_audit(root: &Path, entry: &Value, who: &str) -> Result<String, String> {
    let local = claims::local_date(root).map_err(|err| format!("{who} refused: {err}"))?;
    let dir = root.join(AUDIT_DIR);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return Err(format!("{who} refused: cannot create {AUDIT_DIR}/: {err}"));
    }
    let name = format!("{local}.jsonl");
    let path = dir.join(&name);
    let lock_name = format!("audit-{name}.lock");
    let token = index::acquire_lock(root, &lock_name, who)?;
    let outcome = (|| -> Result<(), String> {
        use std::io::Write;
        let line = format!("{}\n", entry);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|err| format!("{who} refused: cannot open {path:?} for append: {err}"))?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.flush())
            .map_err(|err| format!("{who} refused: appending to {path:?} failed: {err}"))
    })();
    index::release_lock(root, &token, &lock_name, who);
    outcome.map(|()| format!("{AUDIT_DIR}/{name}"))
}

/// Every tick recorded for `check`, from every `.audit/*.jsonl` file, in
/// file order. A missing `.audit/` is simply no ticks; a file that
/// cannot be read is reported loudly (the audit trail is evidence, and
/// an unreadable piece of it must not be quietly skipped), while
/// individual malformed LINES are tolerated (spec §1's tolerance for
/// what it cannot parse, and `last_tick` must survive a junk tail).
fn collect_ticks(root: &Path, check: &str) -> Result<Vec<Value>, String> {
    let dir = root.join(AUDIT_DIR);
    let mut out: Vec<Value> = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut names: Vec<(String, std::path::PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|err| {
        format!("last_tick refused: cannot list {AUDIT_DIR}/: {err}")
    })? {
        let entry = entry.map_err(|err| {
            format!("last_tick refused: listing {AUDIT_DIR}/: {err}")
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".jsonl") || name.starts_with('.') {
            continue;
        }
        if !matches!(entry.file_type(), Ok(ft) if ft.is_file()) {
            continue;
        }
        names.push((name, entry.path()));
    }
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path) in names {
        let text = std::fs::read_to_string(&path).map_err(|err| {
            format!("last_tick refused: cannot read the audit trail {path:?}: {err} — the audit log is compliance evidence, and a piece of it that cannot be read is not evidence of silence")
        })?;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue; // a junk tail line is tolerated, never fatal
            };
            if value.get("check").and_then(Value::as_str) == Some(check) {
                out.push(value);
            }
        }
    }
    Ok(out)
}

/// UTC ISO-8601Z stamp for now. A clock that cannot be read is refused
/// loudly — an entry stamped at the epoch would be aged 56 years and
/// every later reading of it would be a lie.
fn now_utc() -> Result<String, String> {
    Ok(gate::utc_iso8601(now_epoch()? as i64))
}

fn now_epoch() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|err| format!("the system clock is unreadable: {err}"))
}

/// `YYYY-MM-DDTHH:MM:SSZ` -> epoch seconds, `None` when the text is not
/// exactly that shape. The calendar arithmetic is the inverse of the
/// proleptic-Gregorian `civil_from_days` this crate already ships
/// (`config.rs` / `gate.rs`), restated inline — no third-party date
/// dependency (§2), and no `date` subprocess per call.
pub fn parse_utc_iso8601(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() != 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }
    let digits = |mut r: std::ops::Range<usize>| r.all(|i| bytes[i].is_ascii_digit());
    if !(digits(0..4) && digits(5..7) && digits(8..10) && digits(11..13) && digits(14..16) && digits(17..19))
    {
        return None;
    }
    let year: i64 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: u32 = text[11..13].parse().ok()?;
    let minute: u32 = text[14..16].parse().ok()?;
    let second: u32 = text[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60
    {
        return None;
    }
    days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second))
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 for a
/// proleptic Gregorian date, counted from the Unix epoch and re-based to
/// the March-anchored era whose 146097-day repetition puts the century
/// leap rules inside the integer math (the exact inverse of the
/// `civil_from_days` in `config.rs`, verified round-trip in
/// `tests/index_tests.rs`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400; // year of era [0, 399]
    let month = if month > 2 { month - 3 } else { month + 9 };
    let doy = i64::from((153 * month + 2) / 5) + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The `(51m ago)` half of the pinned silence text: minutes inside the
/// hour-scale, hours-and-minutes beyond it. Only the prefix before
/// `(<age> ago)` is pinned; this keeps long silences readable instead of
/// reporting "4320m ago".
fn render_age(secs: i64) -> String {
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    format!("{}h{:02}m ago", minutes / 60, minutes % 60)
}
