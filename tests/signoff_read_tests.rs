//! Wave 2 self-checks — the ExoM2 slice only: the handshake gate
//! (`gate::gate` / `gate::handshake`) and the `read_signoff` parser
//! (warm-start block, `Last updated:` header, ranked lines, History,
//! worker signoffs, and the tool surface).
//!
//! No dependency on the sibling `signoff_append` module — its slice
//! owns its own test file. The crate (and hence these tests)
//! compiles only once BOTH wave-2 modules land; a missing sibling
//! module is the intended loud failure, never something to fix here.

use std::path::PathBuf;

use total_recall::config;
use total_recall::gate;
use total_recall::rpc::{HandlerResult, Server};
use total_recall::tools::signoff_read;
use serde_json::json;

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "total-recall-signoff-read-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A fresh server on a temp root with the given signoff.md content.
fn fresh_server(root: &PathBuf, content: &str) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), content).unwrap();
    Server::new_server(root)
}

/// The full shape: header line, a block with two ranked items plus an
/// unranked prose line, two worker signoffs (one fully stamped, one
/// without the optional fields), and a two-entry History.
const SEED_FULL: &str = r#"# Signoff

Last updated: 2026-09-13 (08:20 PDT) · by this session

## If you read nothing else
1. **First item** — prose one
2. **Second item** — prose two
An unranked prose line the parser must not number.

## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a
Worker signoff (beta) | done: no | unpushed: 3 commits | awaits human: CI label | still running: 2 (vllm eval)

## History
- 2026-09-12 — superseded block prose three
- 2026-09-11 — older superseded block
"#;

#[test]
fn read_signoff_parses_full_seed() {
    let dir = tmp_root("full");
    let mut s = fresh_server(&dir, SEED_FULL);
    let v = match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("read_signoff must succeed: {e}"),
    };
    // `path` + `as_of` (the file's mtime, UTC ISO-8601Z).
    assert_eq!(v["path"], dir.join("signoff.md").display().to_string());
    let as_of = v["as_of"].as_str().unwrap();
    assert!(
        as_of.len() == 20 && as_of.ends_with('Z') && as_of.as_bytes()[10] == b'T',
        "as_of must be UTC ISO-8601Z: {as_of}"
    );
    // The header is the verbatim trimmed `Last updated:` line — the
    // nearest preceding NON-blank line (the seed separates the two
    // with a blank line).
    assert_eq!(
        v["warm_start"]["header"],
        "Last updated: 2026-09-13 (08:20 PDT) · by this session"
    );
    // Ranked: the numbered lines only — the unranked prose is not.
    let ranked = v["warm_start"]["ranked"].as_array().unwrap();
    assert_eq!(ranked.len(), 2, "exactly the numbered lines: {v}");
    assert_eq!(ranked[0]["rank"], 1);
    assert_eq!(ranked[0]["text"], "**First item** — prose one");
    assert_eq!(ranked[1]["rank"], 2);
    assert_eq!(ranked[1]["text"], "**Second item** — prose two");
    // History: leading "- " trimmed, order preserved.
    let history = v["warm_start"]["history"].as_array().unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history[0].as_str().unwrap(),
        "2026-09-12 — superseded block prose three"
    );
    assert_eq!(
        history[1].as_str().unwrap(),
        "2026-09-11 — older superseded block"
    );
    // The stamped line carries all eight fields …
    let ws = v["worker_signoffs"].as_array().unwrap();
    assert_eq!(ws.len(), 2, "both signoff lines parse: {v}");
    let alpha = &ws[0];
    assert_eq!(alpha["worker"], "alpha");
    assert_eq!(alpha["done"], "yes");
    assert_eq!(alpha["unpushed"], "none");
    assert_eq!(alpha["awaits_human"], "none");
    assert_eq!(alpha["still_running"], "no");
    assert_eq!(alpha["kaibo_review"], "n/a (no code changes)");
    assert_eq!(alpha["workflow"], "memory-kernel");
    assert_eq!(alpha["ts"], "2026-09-12T23:41:07Z");
    assert_eq!(alpha["session"], "s-a");
    // … and the unstamped line parses with the optional fields absent.
    let beta = &ws[1];
    assert_eq!(beta["worker"], "beta");
    assert_eq!(beta["done"], "no");
    assert_eq!(beta["unpushed"], "3 commits");
    assert_eq!(beta["awaits_human"], "CI label");
    assert_eq!(beta["still_running"], "2 (vllm eval)");
    assert!(
        beta["kaibo_review"].is_null() && beta["workflow"].is_null()
            && beta["ts"].is_null()
            && beta["session"].is_null(),
        "absent optional fields -> null: {beta}"
    );
    // The successful read granted the handshake.
    assert_eq!(s.handshaken.get("probe").copied(), Some(true));
}

#[test]
fn read_signoff_without_last_updated_line_has_empty_header() {
    let dir = tmp_root("noheader");
    let seed = "# Signoff\n\n## If you read nothing else\n1. **Only item** — prose\n";
    let mut s = fresh_server(&dir, seed);
    let v = match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("read_signoff must succeed: {e}"),
    };
    assert_eq!(
        v["warm_start"]["header"],
        "",
        "no Last updated: line -> empty header"
    );
    assert_eq!(v["warm_start"]["ranked"].as_array().unwrap().len(), 1);
}

#[test]
fn read_signoff_block_without_numbered_lines_has_empty_ranked() {
    let dir = tmp_root("noranked");
    let seed = "# Signoff\n\nLast updated: 2026-09-13 (08:20 PDT) · by this session\n\n## If you read nothing else\nJust prose, no numbered lines.\n";
    let mut s = fresh_server(&dir, seed);
    let v = match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("read_signoff must succeed: {e}"),
    };
    assert!(
        !v["warm_start"]["header"].as_str().unwrap().is_empty(),
        "header present"
    );
    assert!(
        v["warm_start"]["ranked"].as_array().unwrap().is_empty(),
        "no numbered lines -> empty ranked"
    );
}

#[test]
fn read_signoff_tolerates_malformed_worker_lines_by_skipping() {
    let dir = tmp_root("malformed");
    let seed = "# Signoff\n\nLast updated: 2026-09-13 (09:00 PDT) · by this session\n\n## If you read nothing else\n1. **Item** — prose\n\n## Worker sessions — sign off here as you go\n\nWorker signoff (truncated\nWorker signoff () | done: yes | unpushed: x | awaits human: y | still running: z\nWorker signoff (orphan) | done: yes\nnot a signoff line\nWorker signoff (gamma) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: wf-g\n";
    let mut s = fresh_server(&dir, seed);
    let v = match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("malformed lines are skipped, not errors: {e}"),
    };
    let ws = v["worker_signoffs"].as_array().unwrap();
    assert_eq!(ws.len(), 1, "the three malformed lines are skipped: {v}");
    assert_eq!(ws[0]["worker"], "gamma");
    assert_eq!(ws[0]["done"], "yes");
    assert_eq!(ws[0]["unpushed"], "none");
    assert_eq!(ws[0]["awaits_human"], "none");
    assert_eq!(ws[0]["still_running"], "no");
    assert_eq!(ws[0]["workflow"], "wf-g");
}

#[test]
fn read_signoff_on_sectionless_file_succeeds_with_empty_sections() {
    let dir = tmp_root("bare");
    let seed = "# Signoff\n\nProse only, no sections.\n";
    let mut s = fresh_server(&dir, seed);
    let v = match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("a readable file must always read: {e}"),
    };
    assert_eq!(v["warm_start"]["header"], "");
    assert!(v["warm_start"]["ranked"].as_array().unwrap().is_empty());
    assert!(v["warm_start"]["history"].as_array().unwrap().is_empty());
    assert!(v["worker_signoffs"].as_array().unwrap().is_empty());
    assert_eq!(s.handshaken.get("probe").copied(), Some(true));
}

#[test]
fn gate_refuses_before_handshake_counts_and_releases_after() {
    let dir = tmp_root("gate");
    let mut s = fresh_server(&dir, SEED_FULL);
    // Before the handshake: refused, with the exact pinned text,
    // restating the rule at the failure moment (spec §4) …
    assert!(!s.handshaken.contains_key("probe"));
    let err = gate::gate(&mut s, "probe").err().unwrap();
    assert!(
        err.starts_with("handshake incomplete — call read_signoff first"),
        "refusal must carry the exact pinned text: {err}"
    );
    assert!(
        err.contains("read_signoff is the first action of every session"),
        "refusal must restate the rule: {err}"
    );
    // … and every refused attempt is counted at the gate
    // (server-observable; spec §8 test 12).
    assert_eq!(
        s.attempted_write_before_handshake.get("probe").copied(),
        Some(1)
    );
    let _ = gate::gate(&mut s, "probe");
    assert_eq!(
        s.attempted_write_before_handshake.get("probe").copied(),
        Some(2),
        "each refused attempt increments the counter"
    );
    // Sessions are independent: another session is refused on its own.
    gate::gate(&mut s, "other").err().unwrap();
    // read_signoff grants the handshake: marked in the process-local
    // map AND persisted in .state/sessions.json …
    match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("read_signoff must succeed: {e}"),
    }
    assert_eq!(s.handshaken.get("probe").copied(), Some(true));
    assert!(gate::gate(&mut s, "probe").is_ok(), "the gate opens after the handshake");
    assert!(!gate::gate(&mut s, "other").is_ok(), "the gate is per-session");
    // The record persists {session_id, handshaked_at} entries …
    let sessions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".state/sessions.json"))
            .unwrap())
        .unwrap();
    let entries = sessions.as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| e["session_id"] == "probe")
            .count(),
        1,
        "the record persists the session id"
    );
    let entry = entries
        .iter()
        .find(|e| e["session_id"] == "probe")
        .unwrap();
    let stamp = entry["handshaked_at"].as_str().unwrap();
    assert!(
        stamp.len() == 20 && stamp.ends_with('Z'),
        "handshaked_at must be UTC ISO-8601Z: {stamp}"
    );
    // … and idempotently: a second read_signoff re-grants without a
    // duplicate record entry.
    let _ = signoff_read::read_signoff(&mut s, "probe");
    let sessions: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".state/sessions.json"))
            .unwrap())
        .unwrap();
    assert_eq!(
        sessions
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["session_id"] == "probe")
            .count(),
        1,
        "re-handshake must not duplicate the record"
    );
}

#[test]
fn read_signoff_missing_file_refuses_loudly_without_handshake() {
    let dir = tmp_root("nofile");
    config::init_state(&dir).unwrap(); // no signoff.md on this root
    let mut s = Server::new_server(&dir);
    match signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Err(msg) => {
            assert!(msg.contains("read_signoff failed"), "must refuse loudly: {msg}")
        }
        HandlerResult::Ok(_) => panic!("a missing signoff.md must not grant the handshake"),
    }
    assert!(
        s.handshaken.get("probe").copied() != Some(true),
        "no handshake on failure"
    );
    assert!(!dir.join(".state/sessions.json").exists(), "no record on failure");
    assert!(!gate::gate(&mut s, "probe").is_ok(), "the gate stays closed");
}

#[test]
fn tools_and_handlers_advertise_read_signoff_per_spec() {
    let tools = signoff_read::tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "read_signoff");
    assert!(tools[0]
        .description
        .starts_with("MANDATORY FIRST CALL of every session."));
    assert!(tools[0].description.contains("RECORDS the session handshake"));
    assert_eq!(tools[0].input_schema["type"], "object");
    assert_eq!(tools[0].input_schema["additionalProperties"], false);
    assert!(tools[0]
        .input_schema["properties"]
        .as_object()
        .map(|o| o.is_empty())
        .unwrap_or(false));

    let handlers = signoff_read::handlers();
    assert!(handlers.contains_key("read_signoff"));

    // The dispatch entry names the server's own process-local session
    // (spec §2) and grants the handshake through it.
    let dir = tmp_root("handler");
    let mut s = fresh_server(&dir, SEED_FULL);
    let handler = handlers["read_signoff"];
    match handler(&mut s, &json!({})) {
        HandlerResult::Ok(v) => {
            assert!(v["warm_start"].is_object());
            assert_eq!(v["worker_signoffs"].as_array().unwrap().len(), 2);
        }
        HandlerResult::Err(e) => panic!("handler must succeed: {e}"),
    }
    assert_eq!(
        s.handshaken.get(&s.session_id).copied(),
        Some(true),
        "the handler handshakes the server's own session"
    );
}
