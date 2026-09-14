//! Module self-checks for `tools::claims` (wave 3) — the claims-layer
//! contract (local://exo-mcp-wave3-contract.md):
//!
//! - `claim_orchestrator` — disk truth `.claims/orchestrator-<date>`
//!   `{holder_session_id, token, acquired_at}`; fresh claim
//!   `O_CREAT|O_EXCL`; same-session re-claim returns the EXISTING token
//!   and leaves the file byte-identical; a different session on a live
//!   claim is refused with the holder named and the file
//!   byte-identical; `date` defaults to the server's LOCAL date.
//! - `write_dayfile` — gated on the handshake (spec §4) AND on
//!   today's claim via BOTH token checks: the supplied
//!   `orchestrator_token` must equal the token in the claim file for
//!   the LOCAL date AND the server's per-session claim_token for the
//!   caller (stored by that session's own `claim_orchestrator`) — a
//!   claim file planted on disk alone is NOT a valid claim. Any other
//!   outcome is the exact pinned refusal `write_dayfile refused: no
//!   valid orchestrator_<date> claim — single-writer dayfile`,
//!   decided BEFORE any lock is acquired (zero side effects); on
//!   success the day file is replaced byte-exact, atomically
//!   (tmp + rename) under the day file's own `.locks/` lock (the
//!   wave-2 mechanism).
//!
//! WORKER-OWNED (ExoM4a). Deliberately self-contained: these tests
//! exercise only `tools::claims` on top of the wave-1/2 foundation
//! (`gate`, `config`, `rpc`, `tools::signoff_read`) — never the
//! sibling modules. The orchestrator-owned `tests/gate_w3.rs` is the
//! cross-module acceptance gate.
//!
//! Pinned behavior exercised here (the spec §4 GATED set —
//! `claim_orchestrator` is gated like every other disk-writing tool):
//! a non-handshaken session's claim is refused by the gate with the
//! exact pinned text, counted, and touches zero disk; after the
//! handshake the same call lands the disk truth.
//! - Every expected date is derived from `claims::local_date` (the
//!   root's recorded timezone fingerprint, §2) — never hardcoded.

use exomem_mcp::config;
use exomem_mcp::gate::HANDSHAKE_REFUSAL_MESSAGE;
use exomem_mcp::rpc::{HandlerResult, Server};
use exomem_mcp::tools::{claims, signoff_read};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-claims-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The seeded signoff.md — the same seed the orchestrator's gate uses,
/// so the handshake behaves identically here and in `tests/gate_w3.rs`.
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

/// A fully-initialized root where session "probe" has ALREADY
/// handshaken (mirrors the orchestrator gate's setup).
fn seeded(root: &PathBuf) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    let mut s = Server::new_server(root);
    let _ = signoff_read::read_signoff(&mut s, "probe");
    s
}

/// A fully-initialized root where NO session has handshaken — for
/// the gated-claim pins (refusal first, then success after the
/// handshake).
fn bare(root: &PathBuf) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    Server::new_server(root)
}

/// The server's local date for this root — the ONLY way the tests
/// name a date (derived from the root's fingerprint, never a
/// hardcoded literal).
fn date(dir: &PathBuf) -> String {
    claims::local_date(dir).expect("local_date must succeed on an initialized root")
}

/// Claim, or panic with the refusal; returns the full result.
fn claim_value(server: &mut Server, session: &str, args: &Value) -> Value {
    match claims::claim_orchestrator(server, session, args) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("claim as {session} must succeed: {e}"),
    }
}

/// The claim file path for a date.
fn claim_path(dir: &PathBuf, date: &str) -> PathBuf {
    dir.join(".claims").join(format!("orchestrator-{date}"))
}

/// Pre-seed a claim file on disk, bypassing the API — used by the
/// dual-check test: a planted claim whose session has never claimed
/// through the API has no per-session claim_token.
fn seed_claim(dir: &PathBuf, holder: &str, token: &str, date: &str) {
    let claims_dir = dir.join(".claims");
    std::fs::create_dir_all(&claims_dir).unwrap();
    let body = format!(
        r#"{{"holder_session_id":"{holder}","token":"{token}","acquired_at":"2026-09-01T00:00:00Z"}}"#
    );
    std::fs::write(claims_dir.join(format!("orchestrator-{date}")), body).unwrap();
}

#[test]
fn fresh_claim_writes_disk_truth_with_utc_stamp() {
    let dir = tmp_root("claim-fresh");
    let mut s = seeded(&dir);
    let date = date(&dir);

    let v = claim_value(&mut s, "probe", &json!({}));
    let token = v["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty(), "a fresh claim must mint a token");
    assert_eq!(v["claimed"], json!(true));
    assert_eq!(v["reclaimed"], json!(false), "a first claim is not a re-claim");
    assert_eq!(v["date"], json!(date), "the response carries the date the claim was made for");
    assert_eq!(v["holder"], "probe");

    // The DISK is the truth: the file exists under the LOCAL date and
    // carries exactly the recorded fields.
    let cf = claim_path(&dir, &date);
    let raw = std::fs::read_to_string(&cf).unwrap_or_else(|e| panic!("claim file missing at {:?}: {e}", cf));
    let j: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(j["holder_session_id"], "probe");
    assert_eq!(j["token"], token, "the on-disk token must equal the returned token");
    assert!(j["acquired_at"].as_str().unwrap().ends_with('Z'), "acquired_at must be UTC with a trailing Z");
}

#[test]
fn same_session_reclaim_returns_existing_token_file_byte_identical() {
    let dir = tmp_root("claim-re");
    let mut s = seeded(&dir);
    let d = date(&dir);
    let cf = claim_path(&dir, &d);

    let v1 = claim_value(&mut s, "probe", &json!({}));
    let before = std::fs::read(&cf).unwrap();
    let v2 = claim_value(&mut s, "probe", &json!({}));

    assert_eq!(v1["token"], v2["token"], "a same-session re-claim must return the EXISTING token");
    assert_eq!(v2["reclaimed"], json!(true), "the second claim is a re-claim");
    assert_eq!(v1["acquired_at"], v2["acquired_at"], "a re-claim must not re-stamp");
    assert_eq!(std::fs::read(&cf).unwrap(), before, "a re-claim must leave the claim file byte-identical");
}

#[test]
fn other_session_claim_refused_names_holder_file_unchanged() {
    // Mirrors the orchestrator gate: BOTH sessions handshake first
    // (spec §4 — claim_orchestrator is in the GATED set), so the
    // refusal under test is the single-writer rule (step 4), not the
    // handshake gate (step 1).
    let dir = tmp_root("claim-other");
    let mut s = bare(&dir);
    let d = date(&dir);

    // Both sessions handshake BEFORE either claims.
    let _ = signoff_read::read_signoff(&mut s, "holder");
    let _ = signoff_read::read_signoff(&mut s, "intruder");
    let _ = claim_value(&mut s, "holder", &json!({}));
    let cf = claim_path(&dir, &d);
    let before = std::fs::read(&cf).unwrap();

    match claims::claim_orchestrator(&mut s, "intruder", &json!({})) {
        HandlerResult::Err(msg) => {
            assert!(msg.contains("holder"), "refusal must name the holder: {msg}");
            assert!(msg.contains("held"), "refusal must state the single-writer rule: {msg}");
        }
        HandlerResult::Ok(_) => panic!("a different session on a live claim must be refused"),
    }

    assert_eq!(std::fs::read(&cf).unwrap(), before, "the refusal must leave the claim file byte-identical");
    assert!(
        s.attempted_write_before_handshake.is_empty(),
        "the refusal must come from the single-writer rule, not the handshake gate"
    );
}

#[test]
fn claim_without_handshake_refused_counted_zero_disk() {
    // The spec §4 GATED set includes claim_orchestrator: a
    // non-handshaken session is refused by the gate with the exact
    // pinned text, counted, and nothing claim-shaped touches any disk.
    let dir = tmp_root("claim-gate");
    let mut s = bare(&dir);
    assert!(s.handshaken.is_empty(), "setup must be a bare server");

    match claims::claim_orchestrator(&mut s, "holder", &json!({})) {
        HandlerResult::Err(msg) => {
            assert_eq!(msg, HANDSHAKE_REFUSAL_MESSAGE, "the gate's exact pinned refusal");
        }
        HandlerResult::Ok(_) => panic!("a non-handshaken session must be refused by the gate"),
    }
    assert_eq!(
        s.attempted_write_before_handshake.get("holder").copied(),
        Some(1),
        "the refused claim must be counted"
    );
    assert!(!claim_path(&dir, &date(&dir)).exists(), "zero disk: no claim file");
    assert!(!dir.join(".claims").exists(), "zero disk: not even the .claims/ directory");

    // The handshake opens the gate for that session; the same call now
    // lands the disk truth (the gate_w3 shape: handshake first, then
    // claim) — and the successful claim does not disturb the counter.
    let _ = signoff_read::read_signoff(&mut s, "holder");
    let v = claim_value(&mut s, "holder", &json!({}));
    assert!(v["token"].is_string(), "the post-handshake claim mints a token");
    assert_eq!(v["holder"], json!("holder"));
    let j: Value = serde_json::from_str(&std::fs::read_to_string(claim_path(&dir, &date(&dir))).unwrap()).unwrap();
    assert_eq!(j["holder_session_id"], "holder", "the disk truth carries the handshaken holder");
    assert_eq!(
        s.attempted_write_before_handshake.get("holder").copied(),
        Some(1),
        "a later success must not uncount the earlier refusal"
    );
}

#[test]
fn invalid_explicit_date_refused_zero_disk() {
    let dir = tmp_root("claim-baddate");
    let mut s = seeded(&dir);
    for bad in ["2026-02-30", "2026-9-13", "2026-09-131"] {
        match claims::claim_orchestrator(&mut s, "probe", &json!({"date": bad})) {
            HandlerResult::Err(msg) => assert!(msg.contains("date"), "refusal must name the field: {msg}"),
            HandlerResult::Ok(_) => panic!("'{bad}' is not a real YYYY-MM-DD date and must be refused"),
        }
    }
    // A non-string `date` is a schema violation, refused just as loudly.
    match claims::claim_orchestrator(&mut s, "probe", &json!({"date": 5})) {
        HandlerResult::Err(msg) => assert!(msg.contains("date"), "refusal must name the field: {msg}"),
        HandlerResult::Ok(_) => panic!("a non-string date must be refused"),
    }
    assert!(!dir.join(".claims").exists(), "a refused claim must not even create .claims/");
}

#[test]
fn unknown_claim_field_and_non_object_args_refused_zero_disk() {
    let dir = tmp_root("claim-schema");
    let mut s = seeded(&dir);
    match claims::claim_orchestrator(&mut s, "probe", &json!({"date": "2030-01-01", "extra": 1})) {
        HandlerResult::Err(msg) => assert!(msg.contains("unknown"), "refusal must name the violation: {msg}"),
        HandlerResult::Ok(_) => panic!("an unknown field must be refused (additionalProperties: false)"),
    }
    match claims::claim_orchestrator(&mut s, "probe", &json!(7)) {
        HandlerResult::Err(msg) => assert!(msg.contains("object"), "refusal must name the violation: {msg}"),
        HandlerResult::Ok(_) => panic!("non-object arguments must be refused"),
    }
    assert!(!dir.join(".claims").exists(), "refusals must touch no disk at all");
}

#[test]
fn explicit_future_date_claims_under_its_own_name_and_is_never_stale() {
    let dir = tmp_root("claim-future");
    let mut s = seeded(&dir);
    let v = claim_value(&mut s, "probe", &json!({"date": "2030-01-01"}));
    assert_eq!(v["date"], json!("2030-01-01"));
    assert!(dir.join(".claims").join("orchestrator-2030-01-01").exists(), "the claim must live under its own date");

    // A later default-dated claim runs the stale rotation: a FUTURE
    // claim is strictly after today and must stay put.
    let _ = claim_value(&mut s, "probe", &json!({}));
    assert!(
        dir.join(".claims").join("orchestrator-2030-01-01").exists(),
        "a future-dated claim must never be rotated to the archive"
    );
}

#[test]
fn stale_claim_rotates_to_archive_on_next_claim() {
    let dir = tmp_root("claim-rotate");
    let mut s = seeded(&dir);
    let stale = "2020-01-02";

    std::fs::create_dir_all(dir.join(".claims")).unwrap();
    std::fs::write(
        dir.join(".claims").join(format!("orchestrator-{stale}")),
        json!({
            "holder_session_id": "ghost",
            "token": "tok-0000000000000000",
            "acquired_at": "2020-01-02T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    let _ = claim_value(&mut s, "probe", &json!({}));

    assert!(!dir.join(".claims").join(format!("orchestrator-{stale}")).exists(), "the stale claim must be rotated OUT of .claims/");
    assert!(
        dir.join(".claims").join("archive").join(format!("orchestrator-{stale}")).exists(),
        "the stale claim must be archived, not deleted"
    );
}

#[test]
fn write_dayfile_without_claim_refused_exact_text_zero_side_effects() {
    let dir = tmp_root("dayfile-notoken");
    let mut s = seeded(&dir);
    let d = date(&dir);

    match claims::write_dayfile(&mut s, "probe", &json!({"content": "body", "orchestrator_token": "bogus"})) {
        HandlerResult::Err(msg) => {
            let expected = format!("write_dayfile refused: no valid orchestrator_{d} claim — single-writer dayfile");
            assert_eq!(msg, expected, "the refusal must be the EXACT pinned text");
        }
        HandlerResult::Ok(_) => panic!("write_dayfile must refuse without a valid claim"),
    }
    assert!(!dir.join(format!("{d}.md")).exists(), "no day file may be created");
    assert!(!dir.join(".locks").exists(), "the token gate runs BEFORE any lock is acquired");
}

#[test]
fn write_dayfile_wrong_token_refused_claim_unchanged_then_correct_token_succeeds() {
    let dir = tmp_root("dayfile-wrongtok");
    let mut s = seeded(&dir);
    let d = date(&dir);
    let tok = claim_value(&mut s, "probe", &json!({}))["token"].as_str().unwrap().to_string();
    let cf = claim_path(&dir, &d);
    let before = std::fs::read(&cf).unwrap();

    let expected = format!("write_dayfile refused: no valid orchestrator_{d} claim — single-writer dayfile");
    match claims::write_dayfile(&mut s, "probe", &json!({"content": "body", "orchestrator_token": "wrong-token"})) {
        HandlerResult::Err(msg) => assert_eq!(msg, expected, "a wrong token is the same pinned refusal"),
        HandlerResult::Ok(_) => panic!("a wrong token must be refused"),
    }
    assert_eq!(std::fs::read(&cf).unwrap(), before, "the refusal must leave the claim file byte-identical");
    assert!(!dir.join(format!("{d}.md")).exists(), "no day file may be created");
    // The token gate runs BEFORE any lock is acquired: no day-file lock
    // may exist, and the refused write must append NO audit entry — only
    // the successful claim's own entry is on the trail. (The `.locks/`
    // dir itself may exist, emptied, from the claim's acquire/release —
    // it is the LOCK FILE and the audit line that the §8 both-halves
    // rule pins absent, not the directory.)
    if let Ok(rd) = std::fs::read_dir(dir.join(".locks")) {
        for entry in rd {
            let name = entry.unwrap().file_name();
            assert_ne!(
                name.to_str().unwrap(),
                format!("{d}.md.lock"),
                "the token gate runs BEFORE any lock is acquired"
            );
        }
    }
    let audit = dir.join(".audit").join(format!("{d}.jsonl"));
    let trail = std::fs::read_to_string(&audit).expect("the claim's own audit entry exists");
    assert_eq!(
        trail.lines().count(),
        1,
        "the refused write appended no audit entry — only the claim's own"
    );

    // The claim survives the refusal: the correct token still opens the gate.
    match claims::write_dayfile(&mut s, "probe", &json!({"content": "body", "orchestrator_token": tok})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("the correct token must still succeed after a refused attempt: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join(format!("{d}.md"))).unwrap(), "body");
}

#[test]
fn write_dayfile_before_handshake_refused_counted_then_succeeds_after() {
    // A BARE server with NO claim at all: the first write_dayfile
    // is refused by the handshake gate (step 1) — the exact pinned
    // text, counted, zero disk (no day file, no lock, not even a
    // .claims/ directory).
    let dir = tmp_root("dayfile-gate");
    let mut s = bare(&dir);
    let d = date(&dir);
    assert!(s.handshaken.is_empty(), "setup must be a bare server");

    match claims::write_dayfile(&mut s, "probe", &json!({"content": "x", "orchestrator_token": "tok-anything"})) {
        HandlerResult::Err(msg) => assert_eq!(msg, HANDSHAKE_REFUSAL_MESSAGE, "the gate's exact pinned refusal"),
        HandlerResult::Ok(_) => panic!("a non-handshaken session must be refused by the gate"),
    }
    assert_eq!(s.attempted_write_before_handshake.get("probe").copied(), Some(1), "the refused attempt must be counted");
    assert!(!dir.join(format!("{d}.md").to_string()).exists(), "zero bytes: no day file");
    assert!(!dir.join(".locks").exists(), "zero bytes: no lock");
    assert!(!dir.join(".claims").exists(), "zero bytes: not even the claims directory");

    // The handshake opens the gate; the claim via the API — which
    // stores the per-session claim_token — makes the write valid on
    // BOTH legs (claim file + per-session), and the write lands.
    let _ = signoff_read::read_signoff(&mut s, "probe");
    let tok = claim_value(&mut s, "probe", &json!({}))["token"].as_str().unwrap().to_string();
    match claims::write_dayfile(&mut s, "probe", &json!({"content": "x", "orchestrator_token": tok})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("the write must succeed after the handshake and claim: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join(format!("{d}.md"))).unwrap(), "x");
    assert_eq!(s.attempted_write_before_handshake.get("probe").copied(), Some(1), "a later success must not uncount the earlier refusal");
}

#[test]
fn dayfile_planted_claim_file_alone_not_valid_per_session_leg_missing() {
    // The dual token check (amended contract): a claim file planted
    // on disk ALONE is not a valid claim — the server's per-session
    // claim_token (set only by that session's own
    // claim_orchestrator) must match too. "probe" is handshaken but
    // never claimed, so the file leg passes and the per-session leg
    // is missing: the exact pinned refusal, the planted file
    // byte-identical, zero side effects.
    let dir = tmp_root("dayfile-dual");
    let mut s = seeded(&dir);
    let d = date(&dir);
    seed_claim(&dir, "probe", "tok-planted", &d);
    let before = std::fs::read(claim_path(&dir, &d)).unwrap();

    match claims::write_dayfile(&mut s, "probe", &json!({"content": "x", "orchestrator_token": "tok-planted"})) {
        HandlerResult::Err(msg) => {
            assert_eq!(
                msg,
                format!("write_dayfile refused: no valid orchestrator_{d} claim — single-writer dayfile"),
                "a claim file planted on disk alone must draw the exact pinned refusal (the per-session leg is missing)"
            );
        }
        HandlerResult::Ok(_) => panic!("a claim file planted on disk alone must not open the day file"),
    }
    assert_eq!(std::fs::read(claim_path(&dir, &d)).unwrap(), before, "the planted claim file must be byte-identical");
    assert!(!dir.join(format!("{d}.md")).exists(), "zero bytes: no day file");
    assert!(!dir.join(".locks").exists(), "zero bytes: no lock");

    // The claim through the API: same holder, so it re-claims the
    // EXISTING token (file byte-identical) and stores it
    // per-session — now BOTH legs pass and the write lands.
    let v = claim_value(&mut s, "probe", &json!({}));
    assert_eq!(v["token"], "tok-planted", "the re-claim returns the existing token");
    assert_eq!(v["reclaimed"], json!(true));
    match claims::write_dayfile(&mut s, "probe", &json!({"content": "x", "orchestrator_token": "tok-planted"})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("after the API re-claim both legs pass: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join(format!("{d}.md"))).unwrap(), "x");
}

#[test]
fn write_dayfile_valid_token_replaces_byte_exact_no_stray_tmp_lock_released() {
    let dir = tmp_root("dayfile-ok");
    let mut s = seeded(&dir);
    let d = date(&dir);
    let tok = claim_value(&mut s, "probe", &json!({}))["token"].as_str().unwrap().to_string();

    let c1 = "# Day one\n\n- decision one\n- decision two\n";
    match claims::write_dayfile(&mut s, "probe", &json!({"content": c1, "orchestrator_token": tok})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("the day file write must succeed: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join(format!("{d}.md"))).unwrap(), c1, "replacement must be byte-exact");

    // A second replacement: unicode, and NO trailing newline — the
    // file must end exactly where the content ends.
    let c2 = "# Day two ünïcode 中文 (no trailing newline)";
    match claims::write_dayfile(&mut s, "probe", &json!({"content": c2, "orchestrator_token": tok})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("the second day file write must succeed: {e}"),
    }
    assert_eq!(&*std::fs::read(dir.join(format!("{d}.md"))).unwrap(), c2.as_bytes(), "replacement must be byte-exact (no added newline)");

    // The tmp half of the atomic replace must not survive.
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(!name.contains(".tmp-"), "a stray tmp file must not survive the write: {name}");
    }
    // The day file's own lock must be released (the server removes its
    // own lock file).
    assert!(
        !dir.join(".locks").join(format!("{d}.md.lock")).exists(),
        "the dayfile lock must be released after the write"
    );
}

#[test]
fn dayfile_fresh_planted_lock_refused_after_bounded_retry_zero_bytes_not_taken_over() {
    let dir = tmp_root("dayfile-locked");
    let mut s = seeded(&dir);
    let d = date(&dir);
    let tok = claim_value(&mut s, "probe", &json!({}))["token"].as_str().unwrap().to_string();

    // Plant a FRESH lock held by a (simulated) living other writer.
    let locks = dir.join(".locks");
    std::fs::create_dir_all(&locks).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let planted = format!("pid=999999 nonce=424242 epoch={now} session=planter\n");
    std::fs::write(locks.join(format!("{d}.md.lock")), &planted).unwrap();

    let started = std::time::Instant::now();
    match claims::write_dayfile(&mut s, "probe", &json!({"content": "body", "orchestrator_token": tok})) {
        HandlerResult::Err(msg) => {
            assert!(msg.contains(".locks"), "the refusal must name the contested lock: {msg}");
            assert!(msg.contains("held by another writer"), "the refusal must state the lock is live: {msg}");
        }
        HandlerResult::Ok(_) => panic!("a live lock must refuse the write"),
    }
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "the bounded retry must actually retry (a live holder may release): {elapsed:?}");

    assert_eq!(
        std::fs::read(locks.join(format!("{d}.md.lock"))).unwrap(),
        planted.as_bytes(),
        "a FRESH lock must be waited out, never taken over"
    );
    assert!(!dir.join(format!("{d}.md")).exists(), "zero bytes written");
}

#[test]
fn dayfile_stale_planted_lock_taken_over_write_lands_lock_released() {
    let dir = tmp_root("dayfile-stalelock");
    let mut s = seeded(&dir);
    let d = date(&dir);
    let tok = claim_value(&mut s, "probe", &json!({}))["token"].as_str().unwrap().to_string();

    // Plant a STALE lock: the holder has been silent far beyond the
    // takeover threshold.
    let locks = dir.join(".locks");
    std::fs::create_dir_all(&locks).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let planted = format!("pid=999999 nonce=424242 epoch={} session=crashed\n", now - 120);
    std::fs::write(locks.join(format!("{d}.md.lock")), &planted).unwrap();

    let content = "# Recovered day file\n";
    match claims::write_dayfile(&mut s, "probe", &json!({"content": content, "orchestrator_token": tok})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("a STALE lock must be taken over server-side: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join(format!("{d}.md"))).unwrap(), content, "the takeover must let the write land byte-exact");
    assert!(!locks.join(format!("{d}.md.lock")).exists(), "the (new) lock must be released after the write — nothing left behind");
}
