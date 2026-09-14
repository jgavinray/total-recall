//! §8 gate — wave 4 slice (recall + ticks + compliance).
//!
//! ORCHESTRATOR-OWNED per ops/parallel-workers.md invariant 5: written BEFORE the
//! wave launches, workers MUST NOT edit this file. Compile fails loudly until the
//! wave's modules land — that is the intended loud failure.
//!
//! Asserted contract (local://exo-mcp-wave4-contract.md):
//! - `tools::recall::recall` — UNGATED (§4 read/audit-side list); case-insensitive
//!   substring by default, regex mode when the query begins with the literal `re:`
//!   prefix; scopes `dayfile|signoff|briefs|all`; `since` = ISO date lower bound on
//!   each result's ts; `workflow` filter keeps only entries carrying a matching
//!   server-stamped `| workflow: <wf>` tag and drops untagged ones; every result
//!   carries `path` (relative to the root) + `ts` (the file's mtime rendered UTC
//!   ISO-8601Z) + `excerpt` (the matched line, trimmed); path-confined — only the
//!   fixed buckets under the root are ever searched, `wiki/` / `inbox/` / `topics/`
//!   / `signoffs/` are never touched.
//! - `tools::ticks::log_tick` — GATED (handshake, §4 uniform over all five append
//!   paths); appends `{ts, session_id, check, result}` to `.audit/<local-date>.jsonl`
//!   under the audit lock.
//! - `tools::ticks::last_tick` — UNGATED; scans every `.audit/*.jsonl` file and
//!   returns the latest tick for the check; a REGISTERED check whose latest tick has
//!   aged to or past the 45-minute window errors LOUDLY with the exact text
//!   `silence is never good news: check <x> last ticked <ts> (<age> ago)`; an
//!   UNREGISTERED check errors too; the boundary is pinned by seeding a tick at
//!   exactly now-45m (error) and one at now-44m (ok).
//! - `tools::compliance::session_compliance` — UNGATED audit-derived report per
//!   session: `{session_id, handshaked_at, first_write_ts, attempted_write_before_handshake,
//!   write_count, refused_count}` — handshaked_at from `.state/sessions.json`,
//!   first_write_ts = the session's first mutating-tool audit entry ts, write_count =
//!   audit entries count for the session, attempted_write_before_handshake = bool
//!   from the CURRENT process's gate counter (>0), refused_count = the current
//!   process's gate-refusal counter for the session.

use exomem_mcp::config;
use exomem_mcp::rpc::{HandlerResult, Server};
use exomem_mcp::tools::{compliance, recall, ticks};
use serde_json::json;
use std::path::PathBuf;

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-gate-w4-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

const SEED: &str = r#"# Signoff

Last updated: 2026-09-13 (08:20 PDT) · by this session

## If you read nothing else
1. **First item** — prose one
2. **Second item** — prose two

## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a
Worker signoff (beta) | done: no | unpushed: none | awaits human: none | still running: no | workflow: parser-fix | ts: 2026-09-12T20:05:00Z | session: s-b

## History
- 2026-09-12 — superseded block prose three
"#;

fn seeded(root: &PathBuf) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    std::fs::write(root.join("2026-09-13.md"), "# Day\n\nDecision: bring the Rust port up first (reference parity)\nParser edge BLOCKED: version mismatch in the fixture loader\n").unwrap();
    std::fs::create_dir_all(root.join("briefs")).unwrap();
    std::fs::write(root.join("briefs/parser.md"), "# Brief parser\n\nParser edge BLOCKED: version mismatch in the fixture loader\n").unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::write(root.join("wiki/parser-notes.md"), "# Parser notes\n\nParser edge BLOCKED: version mismatch in the fixture loader\n").unwrap();
    let mut s = Server::new_server(root);
    let _ = exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe");
    s
}

fn audit_seed(root: &PathBuf, lines: &[String]) {
    let dir = root.join(".audit");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("2026-09-13.jsonl");
    let mut text = std::fs::read_to_string(&f).unwrap_or_default();
    for l in lines {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&f, text).unwrap();
}

fn tick_line(ts: &str, session: &str, check: &str, result: &str) -> String {
    format!("{{\"ts\":\"{ts}\",\"session_id\":\"{session}\",\"check\":\"{check}\",\"result\":\"{result}\"}}")
}

#[test]
fn recall_works_ungated_before_handshake() {
    let dir = tmp_root("recall-ungated");
    let root = dir.clone();
    config::init_state(&root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    std::fs::write(root.join("2026-09-13.md"), "# Day\n\nDecision: bring the Rust port up first (reference parity)\n").unwrap();
    let mut s = Server::new_server(&root);
    // NO handshake — recall runs ungated per §4.
    let v = match recall::recall(&mut s, "s-fresh", &json!({"query": "Decision"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("recall must run ungated: {e}"),
    };
    assert_eq!(v["count"], 1, "one match over all buckets");
    let first = &v["results"][0];
    assert_eq!(first["path"], "2026-09-13.md");
    assert!(first["ts"].as_str().unwrap().ends_with('Z'), "every result carries a UTC ts");
    assert_eq!(first["excerpt"], "Decision: bring the Rust port up first (reference parity)");
}

#[test]
fn recall_substring_searches_all_buckets_dated_and_confined() {
    let dir = tmp_root("recall-all");
    let mut s = seeded(&dir);
    let v = match recall::recall(&mut s, "probe", &json!({"query": "Parser edge"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("recall must succeed: {e}"),
    };
    let paths: Vec<String> = v["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths.len(), 2, "dayfile + briefs both match");
    assert!(paths.contains(&"2026-09-13.md".to_string()));
    assert!(paths.contains(&"briefs/parser.md".to_string()));
    assert!(!paths.iter().any(|p| p.starts_with("wiki/")), "wiki/ never searched");
    assert_eq!(v["count"], 2);
    for r in v["results"].as_array().unwrap() {
        assert!(r["ts"].as_str().unwrap().ends_with('Z'), "dated excerpt required");
        assert!(r["excerpt"].as_str().unwrap().contains("Parser edge"), "excerpt is the matched line");
    }
}

#[test]
fn recall_scope_confines_to_named_bucket() {
    let dir = tmp_root("recall-scope");
    let mut s = seeded(&dir);
    let v = match recall::recall(&mut s, "probe", &json!({"query": "Parser edge", "scope": "signoff"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("recall must succeed: {e}"),
    };
    let paths: Vec<String> = v["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, vec!["signoff.md"], "scope confines results to the named bucket");
    assert_eq!(v["count"], 1);
}

#[test]
fn recall_regex_mode_matches_alternation() {
    let dir = tmp_root("recall-regex");
    let mut s = seeded(&dir);
    let v = match recall::recall(&mut s, "probe", &json!({"query": "re:Parser edge (BLOCK|BLOCKED)"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("regex mode must succeed: {e}"),
    };
    assert_eq!(v["count"], 2, "regex alternation matched both bucket lines");
}

#[test]
fn recall_workflow_filter_drops_untagged_entries() {
    let dir = tmp_root("recall-wf");
    let mut s = seeded(&dir);
    let v = match recall::recall(&mut s, "probe", &json!({"query": "done:", "workflow": "memory-kernel"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("workflow filter must succeed: {e}"),
    };
    let paths: Vec<String> = v["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, vec!["signoff.md"], "only the tagged signoff entry survives the filter");
    assert_eq!(v["count"], 1);
    assert!(v["results"][0]["excerpt"].as_str().unwrap().contains("| workflow: memory-kernel"), "the surviving line carries the tag");
}

#[test]
fn recall_since_filters_by_date() {
    let dir = tmp_root("recall-since");
    let mut s = seeded(&dir);
    let v = match recall::recall(&mut s, "probe", &json!({"query": "Decision", "since": "2026-09-13"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("since filter must succeed: {e}"),
    };
    let paths: Vec<String> = v["results"].as_array().unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(paths, vec!["2026-09-13.md"], "since lower-bounds the ts date");
}

#[test]
fn log_tick_gated_before_handshake_counts_and_writes_nothing() {
    let dir = tmp_root("tick-gated");
    let mut s = Server::new_server(&dir);
    config::init_state(&dir).unwrap();
    let before = std::fs::read_dir(dir.join(".audit")).map(|d| d.count()).unwrap_or(0);
    let r = ticks::log_tick(&mut s, "probe", &json!({"check": "model-server-health", "result": "ok"}));
    match r {
        HandlerResult::Err(msg) => {
            assert!(msg.contains("handshake incomplete"), "gated refusal text: {msg}");
            assert_eq!(s.attempted_write_before_handshake.get("probe").cloned().unwrap_or(0), 1, "attempt counted once");
        }
        HandlerResult::Ok(_) => panic!("must refuse before the handshake"),
    }
    let after = std::fs::read_dir(dir.join(".audit")).map(|d| d.count()).unwrap_or(0);
    assert_eq!(after, before, "no audit file written");
}

#[test]
fn log_tick_after_handshake_appends_audit_jsonl() {
    let dir = tmp_root("tick-ok");
    let mut s = Server::new_server(&dir);
    config::init_state(&dir).unwrap();
    let _ = exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe");
    match ticks::log_tick(&mut s, "probe", &json!({"check": "model-server-health", "result": "ok"})) {
        HandlerResult::Err(e) => panic!("post-handshake tick must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let f = dir.join(".audit/2026-09-13.jsonl");
    let text = std::fs::read_to_string(&f).unwrap();
    let j: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(j["check"], "model-server-health");
    assert_eq!(j["result"], "ok");
    assert_eq!(j["session_id"], s.session_id);
    assert!(j["ts"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn last_tick_latest_tick_and_silent_check_loud_error() {
    let dir = tmp_root("tick-last");
    let mut s = Server::new_server(&dir);
    config::init_state(&dir).unwrap();
    // seed two ticks: one exactly at the 45-minute boundary, one 44 minutes ago.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let boundary = now - 45 * 60;
    let fresh = now - 44 * 60;
    let iso = |t: i64| {
        // test-only calendar math duplicated inline, VERBATIM from config.rs's
        // civil_from_days (proleptic Gregorian, same algorithm — no third-party
        // date dependency, per §2).
        let z = t / 86_400 + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097; // floor
        let doe = z - era * 146_097; // day of era [0, 146096]
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
        let y = era * 400 + yoe;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let (year, m) = if mp < 10 { (y, mp + 3) } else { (y + 1, mp - 9) };
        let rem = t % 86_400;
        let (hh, mm, ss) = (rem / 3_600, rem / 60 % 60, rem % 60);
        format!("{year:04}-{:02}-{:02}T{hh:02}:{mm:02}:{ss:02}Z", m, day)
    };
    audit_seed(&dir, &[tick_line(&iso(boundary), "s-old", "model-server-health", "ok"),
                       tick_line(&iso(fresh), "s-fresh", "model-server-health", "ok")]);
    let v = match ticks::last_tick(&mut s, "probe", &json!({"check": "model-server-health"})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("fresh tick must pass the window: {e}"),
    };
    assert_eq!(v["check"], "model-server-health");
    assert_eq!(v["ts"], iso(fresh), "latest tick wins");
    // a tick at EXACTLY the 45-minute boundary is aged TO the window -> loud error.
    audit_seed(&dir, &[tick_line(&iso(now - 45 * 60), "s-x", "parser-fix", "ok")]);
    let r = ticks::last_tick(&mut s, "probe", &json!({"check": "parser-fix"}));
    match r {
        HandlerResult::Err(msg) => {
            assert!(msg.starts_with("silence is never good news: check parser-fix last ticked "), "loud error text: {msg}");
        }
        HandlerResult::Ok(_) => panic!("aged-to-window check must error loudly"),
    }
}

#[test]
fn last_tick_unregistered_check_errors_too() {
    let dir = tmp_root("tick-unreg");
    let mut s = Server::new_server(&dir);
    config::init_state(&dir).unwrap();
    let r = ticks::last_tick(&mut s, "probe", &json!({"check": "never-ticked-check"}));
    match r {
        HandlerResult::Err(msg) => assert!(msg.contains("never-ticked-check"), "unregistered check error names the check: {msg}"),
        HandlerResult::Ok(_) => panic!("unregistered check must error"),
    }
}

#[test]
fn compliance_report_is_audit_derived_and_measured() {
    let dir = tmp_root("compliance");
    let mut s = Server::new_server(&dir);
    config::init_state(&dir).unwrap();
    // one REAL refused attempt before the handshake — the gate counter is
    // observable at the gate, and the audit log never records refusals
    // (they land nothing).
    let _ = exomem_mcp::tools::signoff_append::append_signoff(&mut s, "probe", &json!({"role":"r","workflow":"wf","done":"yes"}));
    assert_eq!(s.attempted_write_before_handshake.get("probe").cloned().unwrap_or(0), 1, "counter observable");
    let _ = exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe");
    // two mutating writes AFTER the handshake — each lands an audit entry.
    match exomem_mcp::tools::signoff_append::append_signoff(&mut s, "probe", &json!({"role":"r","workflow":"wf","done":"yes"})) {
        HandlerResult::Err(e) => panic!("post-handshake append must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    match ticks::log_tick(&mut s, "probe", &json!({"check": "c1", "result": "ok"})) {
        HandlerResult::Err(e) => panic!("tick must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let v = match compliance::session_compliance(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("compliance must succeed: {e}"),
    };
    let report = &v["sessions"][0];
    assert_eq!(report["session_id"], s.session_id);
    assert!(report["handshaked_at"].as_str().unwrap().ends_with('Z'));
    assert!(report["first_write_ts"].as_str().unwrap().ends_with('Z'));
    assert_eq!(report["attempted_write_before_handshake"], true, "counter observable");
    assert_eq!(report["write_count"], 2, "audit entries counted (append + tick)");
    assert_eq!(report["refused_count"], 1, "the gate counter observable at the gate");
}
