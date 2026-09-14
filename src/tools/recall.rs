//! The `recall` tool (spec §3) — the only read path with a query in it,
//! and one of the four tools that run UNGATED (spec §4: `read_signoff`,
//! `recall`, `last_tick`, `session_compliance`; everything that touches
//! disk on the write side is gated).
//!
//! Shape (spec §3, verbatim example):
//!
//! ```json
//! {"results": [
//!   {"path": "briefs/parser.md", "ts": "2026-09-12T18:02:11Z", "excerpt": "…"},
//!   {"path": "2026-09-13.md", "ts": "2026-09-13T08:14:02Z", "excerpt": "…"}
//! ], "count": 2}
//! ```
//!
//! Pinned semantics:
//! - **no undated facts**: every result carries `ts` — the LIVE file's
//!   mtime rendered UTC ISO-8601Z, the same mtime convention
//!   `read_signoff` reports as `as_of`;
//! - **case-insensitive substring by default**; the literal prefix `re:`
//!   makes the remainder a case-insensitive regex (§13 R3 — substring /
//!   regex only, no SQLite, no FTS5, no embeddings, no new dependency);
//! - **`scope`** is one of `dayfile|signoff|briefs|all` (default `all`)
//!   and is the ONLY way a bucket is chosen: there is no path parameter
//!   anywhere, the buckets are enumerated from disk under the configured
//!   root, and every candidate is canonicalized and refused if it
//!   resolves outside the root. `wiki/`, `inbox/`, `topics/`,
//!   `signoffs/`, `.audit/`, `.claims/`, `.state/` and `.index/` are not
//!   buckets and are never opened (spec §10, §3 EXCLUSIONS);
//! - **`workflow`** keeps only lines carrying a matching server-stamped
//!   `| workflow: <wf>` tag and drops the untagged ones — the filter
//!   behind the per-workflow separability claim in §7;
//! - **`since`** (ISO date) lower-bounds each result's `ts` date;
//! - the derived index (`crate::tools::index`) narrows the candidate
//!   files and lines, and is never read as fact: each candidate is
//!   re-read and matched against the LIVE bytes, and a candidate the
//!   disk cannot answer for is a LOUD refusal naming the path —
//!   "a check that can't run must fail loudly", never a quiet zero.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::gate;
use crate::rpc::{self, HandlerResult, Server, Tool};
use crate::tools::index::{self, Query, Scope};

/// The tool shape advertised by `tools/list`; the description is the
/// spec §3 text verbatim (descriptions carry the contract — spec §4),
/// and the input schema is spec §3's pinned shape (no path parameter,
/// `additionalProperties: false`).
pub fn tools() -> Vec<Tool> {
    vec![Tool {
        name: "recall".to_string(),
        description: "Use this — instead of trusting context memory — for ANY fact about the past: UNGATED and always available (no handshake needed), it searches the known memory buckets and returns dated excerpts with paths. Every result carries a timestamp/date — no undated facts (perishable-facts rule: facts are perishable, re-verify anything older than its natural rate of change). Facts come from recall() results or files, never from context memory; this answers 'what was true when', while read_signoff answers 'what matters right now'. Not a search engine over arbitrary paths: searches only the fixed buckets under the configured root (§2); the input carries no path parameter and any query-derived path must canonicalize inside those buckets. The server may maintain an internal derived index over the buckets to narrow candidates; the index is never authoritative — every returned excerpt is re-read from the live file and its `ts` is that file's mtime. A stale, corrupt, or version-mismatched index is rebuilt, never served.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search terms; case-insensitive substring by default — when it begins with the literal prefix `re:` the remainder is treated as a case-insensitive regex (§13 R3)"},
                "scope": {"type": "string", "description": "Bucket to search; omit (or 'all') to search every bucket", "enum": ["dayfile", "signoff", "briefs", "all"], "default": "all"},
                "workflow": {"type": "string", "description": "Filter to entries whose server-stamped `workflow:` field matches (signoff entries and audit-derived records carry it); results without a workflow tag are excluded when the filter is set. This filter is what backs the per-workflow separability claim in §7."},
                "since": {"type": "string", "description": "ISO date filter (YYYY-MM-DD); omit for full range"}
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    }]
}

/// The dispatch entry this module owns: the `tools/call` entry point
/// for `recall`. The adapter names the server's own process-local
/// session (spec §2); the contract entry point [`recall`] takes the
/// session explicitly so the gate tests can drive named sessions — and
/// ignores it, because recall is ungated (spec §4).
pub fn handlers() -> HashMap<String, rpc::Handler> {
    let mut handlers: HashMap<String, rpc::Handler> = HashMap::new();
    handlers.insert("recall".to_string(), recall_handler);
    handlers
}

/// The `tools/call` dispatch entry (see [`handlers`]).
fn recall_handler(server: &mut Server, arguments: &Value) -> HandlerResult {
    let session = server.session_id.clone();
    recall(server, &session, arguments)
}

/// Search the fixed buckets for `query` (the contract entry point).
///
/// Order of operations, pinned so that nothing is ever served from
/// something the server did not just read:
/// 1. argument validation against the pinned schema — every refusal
///    names the field and the shape it wanted, and touches no disk;
/// 2. bucket enumeration (path confinement — an escaping symlink is a
///    refusal naming the path);
/// 3. validated snapshot from the derived index — a fingerprint
///    mismatch, a corrupt or version-mismatched report, or a plain miss
///    is a SILENT rebuild from disk; a rebuild that cannot run is a
///    LOUD refusal naming the path;
/// 4. the index narrows candidate files and lines;
/// 5. every candidate is RE-READ and matched against the live bytes,
///    and its `ts` is its live mtime. An unreadable candidate is a loud
///    refusal, never a silent omission: a match the server cannot
///    verify is not the same thing as a match that is not there.
pub fn recall(server: &mut Server, _session: &str, arguments: &Value) -> HandlerResult {
    // 1. arguments — strict: the pinned schema admits exactly four
    // fields, and `query` must be there.
    let args = match arguments.as_object() {
        Some(map) => map,
        None => {
            return HandlerResult::Err(
                "recall refused: arguments must be a JSON object ({\"query\": …})".to_string(),
            )
        }
    };
    // additionalProperties: false — an unknown field is refused and named,
    // never quietly ignored. This matters more here than anywhere else:
    // the schema has deliberately NO path parameter, so an escape attempt
    // arrives as a field and must be refused as one.
    let unknown: Vec<&str> = args
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "query" | "scope" | "workflow" | "since"
            )
        })
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return HandlerResult::Err(format!(
            "recall refused: unknown field(s) {unknown:?} — the schema allows only: query, scope, workflow, since"
        ));
    }
    let query = match args.get("query") {
        None => {
            return HandlerResult::Err(
                "recall refused: missing required field 'query'".to_string(),
            )
        }
        Some(value) => match value.as_str() {
            Some(text) if !text.is_empty() => text,
            Some(_) => {
                return HandlerResult::Err(
                    "recall refused: field 'query' must not be empty — a search with no terms is a bucket dump, which is not what recall is for"
                        .to_string(),
                )
            }
            None => {
                return HandlerResult::Err(format!(
                    "recall refused: field 'query' must be a string (got {value})"
                ))
            }
        },
    };
    let scope = match args.get("scope") {
        None | Some(Value::Null) => Scope::All,
        Some(value) => match value.as_str() {
            Some("dayfile") => Scope::Dayfile,
            Some("signoff") => Scope::Signoff,
            Some("briefs") => Scope::Briefs,
            Some("all") => Scope::All,
            Some(other) => {
                return HandlerResult::Err(format!(
                    "recall refused: scope must be one of dayfile|signoff|briefs|all (got {other:?})"
                ))
            }
            None => {
                return HandlerResult::Err(format!(
                    "recall refused: field 'scope' must be a string (got {value})"
                ))
            }
        },
    };
    let workflow = match args.get("workflow") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_str() {
            Some(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            Some(_) => {
                return HandlerResult::Err(
                    "recall refused: field 'workflow' must not be empty — it is a filter, not a wildcard".to_string(),
                )
            }
            None => {
                return HandlerResult::Err(format!(
                    "recall refused: field 'workflow' must be a string (got {value})"
                ))
            }
        },
    };
    let since = match args.get("since") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_str() {
            Some(text) if is_iso_date(text) => Some(text.to_string()),
            Some(other) => {
                return HandlerResult::Err(format!(
                    "recall refused: since must be an ISO date YYYY-MM-DD (got {other:?})"
                ))
            }
            None => {
                return HandlerResult::Err(format!(
                    "recall refused: field 'since' must be a string (got {value})"
                ))
            }
        },
    };

    // The matcher is compiled BEFORE any search runs: a malformed
    // `re:` pattern is refused here, loudly, not answered as the empty
    // set a broken search would produce.
    let query = match Query::compile(query) {
        Ok(compiled) => compiled,
        Err(err) => {
            return HandlerResult::Err(format!("recall refused: {err} (query: {query:?})"))
        }
    };

    // 2. buckets — enumerated, never asked for.
    let root = server.root.as_path();
    let files = match index::bucket_files(root, scope) {
        Ok(files) => files,
        Err(err) => return HandlerResult::Err(err),
    };
    if files.is_empty() {
        // A fresh root is empty (spec §2); an empty answer from an
        // empty bucket is honest, not a failure.
        return HandlerResult::Ok(json!({"results": [], "count": 0}));
    }

    // 3. the derived index — validated against the live files on this
    // very call.
    let snapshot = match index::snapshot_with(root, true) {
        Ok(snapshot) => snapshot,
        Err(err) => return HandlerResult::Err(err),
    };

    // 4. candidates (a hint about where a match can be, nothing more).
    let candidates = match snapshot.candidates(&files, &query) {
        Ok(candidates) => candidates,
        Err(err) => return HandlerResult::Err(format!("recall refused: {err}")),
    };
    if candidates.is_empty() {
        return HandlerResult::Ok(json!({"results": [], "count": 0}));
    }

    // 5. verify against the live bytes. The indexed line numbers are
    // never returned as excerpts and never trusted as the last word:
    // the live file decides, and its mtime is the stamp.
    let mut results: Vec<Value> = Vec::new();
    for candidate in candidates {
        let text = match std::fs::read_to_string(&candidate.file.abs) {
            Ok(text) => text,
            Err(err) => {
                return HandlerResult::Err(format!(
                    "recall refused: cannot re-read candidate bucket file {:?} to verify it ({} indexed lines matched; nothing is served that the live file has not confirmed): {err}",
                    candidate.file.abs,
                    candidate.lines.len()
                ))
            }
        };
        let live_mtime = match std::fs::metadata(&candidate.file.abs).and_then(|m| m.modified()) {
            Ok(mtime) => match mtime.duration_since(std::time::UNIX_EPOCH) {
                Ok(since_epoch) => since_epoch.as_secs() as i64,
                Err(err) => {
                    return HandlerResult::Err(format!(
                        "recall refused: bucket file {:?} is stamped before the Unix epoch — an undated fact is worse than no fact: {err}",
                        candidate.file.abs
                    ))
                }
            },
            Err(err) => {
                return HandlerResult::Err(format!(
                    "recall refused: cannot stat bucket file {:?} — every result must carry its file's mtime, and an undated excerpt is the one thing recall is forbidden to return: {err}",
                    candidate.file.abs
                ))
            }
        };
        let ts = gate::utc_iso8601(live_mtime);
        if let Some(bound) = &since {
            // ISO dates compare correctly as strings; `ts` is UTC
            // ISO-8601Z, so its first ten characters are the date.
            if &ts[..10] < bound.as_str() {
                continue;
            }
        }
        for line in index::split_lines(&text) {
            let matched = match query.matches(&line) {
                Ok(matched) => matched,
                Err(err) => return HandlerResult::Err(format!("recall refused: {err}")),
            };
            if !matched {
                continue;
            }
            let excerpt = line.trim();
            if excerpt.is_empty() {
                continue;
            }
            if let Some(wanted) = &workflow {
                // The filter keeps tagged entries and DROPS untagged
                // ones (spec §3): a line without a server-stamped
                // workflow tag can never satisfy the filter.
                match workflow_tag(&line) {
                    Some(tag) if tag == *wanted => {}
                    _ => continue,
                }
            }
            results.push(json!({
                "path": candidate.file.rel,
                "ts": ts,
                "excerpt": excerpt,
            }));
        }
    }

    // Deterministic order without a sort: candidates arrive in bucket
    // enumeration order (signoff, then day files by name, then briefs
    // by name), and matched lines in file order.
    let count = results.len();
    HandlerResult::Ok(json!({"results": results, "count": count}))
}

/// The server-stamped workflow tag on a line, if it carries one: the
/// ` | `-separated field `workflow: <value>` (the form
/// `append_signoff` writes and `read_signoff` parses). The field splits
/// at the FIRST `:`, so a value may contain colons.
fn workflow_tag(line: &str) -> Option<String> {
    line.split('|').find_map(|field| {
        let field = field.trim();
        let (key, value) = field.split_once(':')?;
        if key.trim().eq_ignore_ascii_case("workflow") && !value.trim().is_empty() {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

/// `YYYY-MM-DD`: four digits, `-`, two, `-`, two, with sane fields.
/// Anything else (`2026-9-13`, `last week`, an absolute path posing as
/// a date) is refused rather than silently searched without a bound.
fn is_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    if !bytes.iter().enumerate().all(|(i, b)| match i {
        4 | 7 => *b == b'-',
        _ => b.is_ascii_digit(),
    }) {
        return false;
    }
    let month: u32 = text[5..7].parse().unwrap_or(0);
    let day: u32 = text[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}
