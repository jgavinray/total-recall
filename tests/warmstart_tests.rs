//! §8 gate — `write_warm_start` (wave 3): the worker-owned scoped
//! tests.
//!
//! These tests deliberately do NOT touch the sibling wave-3 `claims`
//! module: claim state is seeded directly — the on-disk claim record
//! (`.claims/orchestrator-<date>`) plus the server's process-local
//! `claim_tokens` entry — which is exactly the state
//! `claim_orchestrator` would have produced, so the two modules can
//! be verified independently (ops/parallel-workers.md invariant 5).
//! The SEED fixture and the `local_date` derivation are copies of
//! the gate's own (its `SEED`/`local_date` are private to that test
//! crate).
//!
//! Pinned behaviors under test (wave-3 contract lines 50-57; spec
//! §3/§4/§5/§8):
//! - pre-handshake refusal is the gate's pinned message, counted
//!   exactly once, and touches no disk (no lock file);
//! - no claim / a claim file planted on disk alone / the presented
//!   token disagreeing with the claim file / the session's stored
//!   token disagreeing with the claim file → refusal naming
//!   claim/token, signoff.md byte-identical, NO lock file created
//!   (the claim gate is decided before any lock is acquired);
//! - bad arguments are refused before the claim gate and the lock;
//! - a full rewrite: the exact expected bytes — the new region
//!   replaces the old one (heading + its `Last updated:` header
//!   line), every other byte verbatim, the superseded region moved
//!   into `## History` as `- ` entries, the result JSON reports the
//!   new region's line count, and no lock file or tmp file survives;
//! - a missing warm-start region or a missing signoff.md → refusal,
//!   nothing written, no lock file, no tmp file;
//! - a file with no `## History` section gets one created at EOF;
//!   a History section followed by another `## ` section ends the
//!   insertion at that boundary;
//! - a `content` that does not end with a newline is normalized so
//!   the region boundary stays clean (the file ends with a newline,
//!   the byte-exact expectation is unchanged).

use exomem_mcp::config;
use exomem_mcp::rpc::{HandlerResult, Server};
use exomem_mcp::tools::warmstart;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-warmstart-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The 14-line seed signoff.md — the gate's own SEED fixture, copied
/// (its const is private to the gate test crate). Line indices: 0 `#
/// Signoff`, 2 `Last updated: …`, 4 `## If you read nothing else`,
/// 8 `## Worker sessions …`, 12 `## History` (blanks at 1, 3, 7, 9,
/// 11).
const SEED: &str = r#"# Signoff

Last updated: 2026-09-13 (08:20 PDT) · by this session

## If you read nothing else
1. **First item** — prose one
2. **Second item** — prose two

## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a

## History
- 2026-09-12 — superseded block prose three
"#;

/// The exact expected file after rewriting the SEED's region with
/// the gate's new content (the gate's success test asserts the
/// contains-projections of this; here the WHOLE file is pinned):
/// head + new region (4 lines) + the verbatim middle (worker
/// section) + the History section + the superseded region rendered
/// as `- ` entries (non-blank lines of the old region).
const EXPECTED: &str = r#"# Signoff

Last updated: 2026-09-13

## If you read nothing else
1. **New item** — fresh prose
## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a

## History
- 2026-09-12 — superseded block prose three
- Last updated: 2026-09-13 (08:20 PDT) · by this session
- ## If you read nothing else
- 1. **First item** — prose one
- 2. **Second item** — prose two
"#;

/// The server's LOCAL date — the same derivation the gate uses
/// (`config::resolve_tz()` + Howard Hinnant civil-from-days integer
/// math), so the claim-file name these tests plant is never pinned
/// to a date the box's local calendar can drift past.
fn local_date() -> String {
    let (_, offset) = config::resolve_tz().unwrap();
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 + offset * 60;
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
    format!("{year:04}-{:02}-{:02}", m, day)
}

/// The gate's fixture: fresh tmp root + init fingerprint + the SEED
/// signoff.md + the `probe` session's handshake.
fn seeded(root: &PathBuf) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    let mut s = Server::new_server(root);
    match exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("handshake must succeed on the SEED fixture: {e}"),
    }
    s
}

/// Plant today's claim record on disk — the file
/// `claim_orchestrator` would write (all three fields, the on-disk
/// truth). These tests do not call the sibling module.
fn plant_claim_file(root: &Path, session: &str, token: &str) {
    let dir = root.join(".claims");
    std::fs::create_dir_all(&dir).unwrap();
    let v = json!({
        "holder_session_id": session,
        "token": token,
        "acquired_at": "2026-09-14T00:00:00Z"
    });
    std::fs::write(dir.join(format!("orchestrator-{}", local_date())), v.to_string()).unwrap();
}

/// The full claim state: the on-disk record AND the server's
/// per-session stored token (what `claim_orchestrator` records after
/// writing the file) — and the presented token all three legs
/// compare against.
fn full_claim(root: &Path, session: &str, token: &str, s: &mut Server) {
    plant_claim_file(root, session, token);
    s.claim_tokens.insert(session.to_string(), token.to_string());
}

/// The shared refusal assertion: Err naming the claim leg,
/// signoff.md byte-identical, no lock file created (the claim gate
/// is decided before any lock is acquired).
fn assert_refused(r: HandlerResult, dir: &Path, before: &[u8]) {
    match r {
        HandlerResult::Err(msg) => {
            assert!(
                msg.contains("claim") || msg.contains("token"),
                "the refusal must name the claim leg: {msg}"
            );
        }
        HandlerResult::Ok(_) => panic!("must refuse"),
    }
    assert_eq!(std::fs::read(dir.join("signoff.md")).unwrap(), before, "signoff.md byte-identical");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "no lock file on a pre-lock refusal");
}

#[test]
fn warmstart_pre_handshake_refused_and_counted() {
    let dir = tmp_root("warm-nohand");
    config::init_state(&dir).unwrap();
    std::fs::write(dir.join("signoff.md"), SEED).unwrap();
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let mut s = Server::new_server(&dir);
    // No read_signoff: the gate is closed for "probe".
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"t"}),
    );
    match r {
        HandlerResult::Err(msg) => assert!(
            msg.contains("handshake"),
            "pre-handshake refusal is the gate's pinned message: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("must refuse before the handshake"),
    }
    assert_eq!(
        s.attempted_write_before_handshake.get("probe").copied(),
        Some(1),
        "one refused attempt is counted exactly once (a double gate would tick it twice)"
    );
    assert_eq!(std::fs::read(dir.join("signoff.md")).unwrap(), before, "signoff.md byte-identical");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "no lock file before the gate opens");
}

#[test]
fn warmstart_without_claim_refused_file_unchanged() {
    let dir = tmp_root("warm-noclaim");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"bogus"}),
    );
    assert_refused(r, &dir, &before);
}

#[test]
fn warmstart_planted_claim_file_alone_refused() {
    let dir = tmp_root("warm-planted");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    // The claim file exists on disk, but this session's per-session
    // token is not set: a file planted on disk alone is not a claim.
    plant_claim_file(&dir, "probe", "planted-tok");
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"planted-tok"}),
    );
    assert_refused(r, &dir, &before);
}

#[test]
fn warmstart_presented_token_differs_from_claim_file_refused() {
    let dir = tmp_root("warm-presentswap");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    plant_claim_file(&dir, "probe", "file-tok");
    s.claim_tokens
        .insert("probe".to_string(), "file-tok".to_string());
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"other-tok"}),
    );
    assert_refused(r, &dir, &before);
}

#[test]
fn warmstart_session_stored_token_differs_from_file_refused() {
    let dir = tmp_root("warm-storeswap");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    plant_claim_file(&dir, "probe", "file-tok");
    s.claim_tokens
        .insert("probe".to_string(), "session-tok".to_string());
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"file-tok"}),
    );
    assert_refused(r, &dir, &before);
}

#[test]
fn warmstart_success_exact_bytes_and_no_leftovers() {
    let dir = tmp_root("warm-ok");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-warm", &mut s);
    let content = "Last updated: 2026-09-13\n\n## If you read nothing else\n1. **New item** — fresh prose\n";
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content": content, "orchestrator_token": "t-warm"}),
    );
    match r {
        HandlerResult::Ok(v) => {
            assert_eq!(v["rewritten"], json!("signoff.md"));
            assert_eq!(v["warm_start_lines"], json!(4));
            assert_eq!(v["history_blocks_moved"], json!(1));
            assert_eq!(v["tail_bytes_preserved"], json!(true));
        }
        HandlerResult::Err(e) => panic!("the rewrite must succeed: {e}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("signoff.md")).unwrap(),
        EXPECTED,
        "every byte outside the rewritten region is the original's; the region is the caller's"
    );
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".signoff.md.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no tmp file survives the rename: {leftovers:?}");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "the lock is released");
}

#[test]
fn warmstart_missing_region_refused() {
    let dir = tmp_root("warm-noregion");
    config::init_state(&dir).unwrap();
    std::fs::write(
        dir.join("signoff.md"),
        "# Signoff\n\n## Workers\nW1\n",
    )
    .unwrap();
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let mut s = Server::new_server(&dir);
    match exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("handshake: {e}"),
    }
    full_claim(&dir, "probe", "t-warm", &mut s);
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"t-warm"}),
    );
    match r {
        HandlerResult::Err(msg) => assert!(!msg.is_empty(), "a missing region must refuse loudly"),
        HandlerResult::Ok(_) => panic!("a file with no warm-start region must refuse"),
    }
    assert_eq!(std::fs::read(dir.join("signoff.md")).unwrap(), before, "signoff.md byte-identical");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "no lock file left behind");
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".signoff.md.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no tmp file left behind: {leftovers:?}");
}

#[test]
fn warmstart_missing_file_refused_no_file_created() {
    let dir = tmp_root("warm-nofile");
    config::init_state(&dir).unwrap();
    let mut s = Server::new_server(&dir);
    // Handshake first (read_signoff needs the file to exist), then
    // delete it: the in-process handshake survives, so the call
    // reaches the under-lock read — where the absence is detected.
    std::fs::write(dir.join("signoff.md"), SEED).unwrap();
    match exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("handshake: {e}"),
    }
    std::fs::remove_file(dir.join("signoff.md")).unwrap();
    full_claim(&dir, "probe", "t-warm", &mut s);
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"new warm","orchestrator_token":"t-warm"}),
    );
    match r {
        HandlerResult::Err(msg) => assert!(!msg.is_empty(), "a missing signoff.md must refuse loudly"),
        HandlerResult::Ok(_) => panic!("an absent signoff.md has no region to rewrite"),
    }
    assert!(!dir.join("signoff.md").exists(), "no signoff.md is created");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "no lock file left behind");
    let leftovers: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".signoff.md.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no tmp file left behind: {leftovers:?}");
}

#[test]
fn warmstart_bad_args_refused_before_lock() {
    let dir = tmp_root("warm-badargs");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    full_claim(&dir, "probe", "t-warm", &mut s);
    for args in [
        json!({"content":"x"}),
        json!({"content":"x","orchestrator_token":"t-warm","extra":1}),
        json!({"content":"   ","orchestrator_token":"t-warm"}),
        json!({"content":1,"orchestrator_token":"t-warm"}),
    ] {
        let r = warmstart::write_warm_start(&mut s, "probe", &args);
        match r {
            HandlerResult::Err(msg) => assert!(!msg.is_empty(), "bad args {args} must be refused"),
            HandlerResult::Ok(_) => panic!("bad args {args} must be refused"),
        }
    }
    assert_eq!(std::fs::read(dir.join("signoff.md")).unwrap(), before, "signoff.md byte-identical");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "the args gate runs before the lock is taken");
}

#[test]
fn warmstart_history_absent_created_at_eof() {
    let dir = tmp_root("warm-nohist");
    config::init_state(&dir).unwrap();
    std::fs::write(
        dir.join("signoff.md"),
        "# Signoff\n\nLast updated: 2026-09-14\n\n## If you read nothing else\n- old item one\n- old item two\n\n## Worker sessions — sign off here as you go\nWorker signoff (beta) | done: yes | session: s-b\n",
    )
    .unwrap();
    let mut s = Server::new_server(&dir);
    match exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("handshake: {e}"),
    }
    full_claim(&dir, "probe", "t-warm", &mut s);
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content":"## If you read nothing else\n- new item\n","orchestrator_token":"t-warm"}),
    );
    match r {
        HandlerResult::Ok(v) => assert_eq!(v["warm_start_lines"], json!(2)),
        HandlerResult::Err(e) => panic!("a missing History section is created at EOF: {e}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("signoff.md")).unwrap(),
        r#"# Signoff

## If you read nothing else
- new item
## Worker sessions — sign off here as you go
Worker signoff (beta) | done: yes | session: s-b
## History
- Last updated: 2026-09-14
- ## If you read nothing else
- - old item one
- - old item two
"#
    );
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "the lock is released");
}

#[test]
fn warmstart_history_ends_at_next_heading() {
    let dir = tmp_root("warm-tail");
    config::init_state(&dir).unwrap();
    std::fs::write(
        dir.join("signoff.md"),
        "# Signoff\n\nLast updated: 2026-09-14 (10:00 PDT)\n\n## If you read nothing else\n1. **Old** — prose\n\n## Worker sessions — sign off here as you go\nWorker signoff (gamma) | done: no | session: s-c\n\n## History\n- 2026-09-01 — an earlier entry\n\n## Tail section\ntail prose preserved\n",
    )
    .unwrap();
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let mut s = Server::new_server(&dir);
    match exomem_mcp::tools::signoff_read::read_signoff(&mut s, "probe") {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("handshake: {e}"),
    }
    full_claim(&dir, "probe", "t-warm", &mut s);
    let content = "## If you read nothing else\n2. **Current** — state\n";
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content": content, "orchestrator_token": "t-warm"}),
    );
    match r {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("the rewrite must succeed: {e}"),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert_eq!(
        text,
        r#"# Signoff

## If you read nothing else
2. **Current** — state
## Worker sessions — sign off here as you go
Worker signoff (gamma) | done: no | session: s-c

## History
- 2026-09-01 — an earlier entry
- Last updated: 2026-09-14 (10:00 PDT)
- ## If you read nothing else
- 1. **Old** — prose

## Tail section
tail prose preserved
"#
    );
    assert!(text.contains("tail prose preserved"), "the bytes after the History section survive verbatim");
    let _ = before;
}

#[test]
fn warmstart_content_without_trailing_newline_normalized() {
    let dir = tmp_root("warm-notrail");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-warm", &mut s);
    // The same block as the gate's success content, MINUS the final
    // newline: the tool must normalize the region boundary instead of
    // fusing the new block with the next heading.
    let content = "Last updated: 2026-09-13\n\n## If you read nothing else\n1. **New item** — fresh prose";
    let r = warmstart::write_warm_start(
        &mut s,
        "probe",
        &json!({"content": content, "orchestrator_token": "t-warm"}),
    );
    match r {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("a missing trailing newline is normalized, not refused: {e}"),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert!(
        text.contains("1. **New item** — fresh prose\n## Worker sessions"),
        "the new region is newline-terminated at the boundary"
    );
    assert!(text.ends_with('\n'), "the file ends with a newline");
    assert_eq!(text, EXPECTED, "normalization changes no other byte");
    assert!(!dir.join(".locks").join("signoff.md.lock").exists(), "the lock is released");
}
