//! §8 gate — `write_brief` (wave 3): the worker-owned scoped tests.
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
//! Pinned behaviors under test (wave-3 contract lines 39-49; spec
//! §3/§4/§5/§8):
//! - pre-handshake refusal is the gate's pinned message, counted
//!   exactly once, and touches no disk;
//! - no claim / a claim file planted on disk alone / a per-session
//!   token that disagrees with the claim file → refusal naming
//!   claim+token, ZERO bytes written, no `briefs/` bucket created;
//! - an unsafe `worker` segment is refused BEFORE the bucket exists;
//! - bad arguments (unknown / missing / non-string fields, non-object)
//!   are refused before any disk work;
//! - the first write is byte-exact and archives nothing
//!   (`archived_to` is null);
//! - a superseding write archives the superseded brief under
//!   `briefs/<worker>-<date>T<HHMMSS>Z.md` stamped with the
//!   SUPERSEDED CONTENT'S mtime (pinned by setting the mtime to a
//!   known epoch — no sleep, no flake), and the rename preserves the
//!   old bytes;
//! - a same-mtime-second collision with a DIFFERENT existing archive
//!   → refused, zero side effects (no archive is ever overwritten);
//! - a byte-identical re-arrival is idempotent: the existing archive
//!   is kept and re-reported, never rewritten.

use exomem_mcp::config;
use exomem_mcp::rpc::{HandlerResult, Server};
use exomem_mcp::tools::briefs;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-briefs-{name}-{}", std::process::id()));
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

/// Plant today's claim record on disk — the file `claim_orchestrator`
/// would write (all three fields, the on-disk truth). These tests do
/// not call the sibling module.
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
/// writing the file) — both legs the tool validates.
fn full_claim(root: &Path, session: &str, token: &str, s: &mut Server) {
    plant_claim_file(root, session, token);
    s.claim_tokens.insert(session.to_string(), token.to_string());
}

/// The fixed reference second: 2026-01-01T00:00:00Z — rendered in an
/// archive-name stamp as `2026-01-01T000000Z`. Setting the superseded
/// file's mtime to this pins the archive name deterministically (no
/// sleep, no second-boundary flake).
const FIXED_EPOCH: u64 = 1_767_225_600;

/// Pin the mtime of `path` to `UNIX_EPOCH + epoch_secs`.
fn set_mtime(path: &Path, epoch_secs: u64) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_secs(epoch_secs))
        .unwrap();
}

#[test]
fn brief_pre_handshake_refused_and_counted() {
    let dir = tmp_root("brief-nohand");
    config::init_state(&dir).unwrap();
    std::fs::write(dir.join("signoff.md"), SEED).unwrap();
    let mut s = Server::new_server(&dir);
    // No read_signoff: the gate is closed for "probe".
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":"body"}));
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
    assert!(!dir.join("briefs").exists(), "no briefs/ bucket on refusal");
    assert!(!dir.join("briefs/alpha.md").exists(), "no brief written");
}

#[test]
fn brief_without_claim_refused_no_files() {
    let dir = tmp_root("brief-noclaim");
    let mut s = seeded(&dir);
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":"body"}));
    match r {
        HandlerResult::Err(msg) => assert!(
            msg.contains("claim") && msg.contains("token"),
            "the refusal must name the claim leg: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("must refuse with no claim"),
    }
    assert!(!dir.join("briefs").exists(), "no briefs/ bucket created");
}

#[test]
fn brief_planted_claim_file_alone_refused() {
    let dir = tmp_root("brief-planted");
    let mut s = seeded(&dir);
    // The claim file exists on disk, but this session's per-session
    // token is not set: a file planted on disk alone is not a claim.
    plant_claim_file(&dir, "probe", "planted-tok");
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":"body"}));
    match r {
        HandlerResult::Err(msg) => assert!(
            msg.contains("claim") && msg.contains("token"),
            "a planted file alone must be refused, naming the claim leg: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("a claim file planted on disk alone is not a claim"),
    }
    assert!(!dir.join("briefs").exists(), "no briefs/ bucket created");
}

#[test]
fn brief_token_mismatch_refused() {
    let dir = tmp_root("brief-mismatch");
    let mut s = seeded(&dir);
    plant_claim_file(&dir, "probe", "file-tok");
    s.claim_tokens
        .insert("probe".to_string(), "session-tok".to_string());
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":"body"}));
    match r {
        HandlerResult::Err(msg) => assert!(
            msg.contains("claim") && msg.contains("token"),
            "the two legs must agree — refusal names the claim: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("claim file and session token disagree: must refuse"),
    }
    assert!(!dir.join("briefs").exists(), "no briefs/ bucket created");
}

#[test]
fn brief_first_write_byte_exact_no_archive() {
    let dir = tmp_root("brief-first");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    let body = "# Brief alpha v1\n";
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":body}));
    match r {
        HandlerResult::Ok(v) => {
            assert_eq!(v["path"], json!("briefs/alpha.md"));
            assert!(v["archived_to"].is_null(), "the first write supersedes nothing");
        }
        HandlerResult::Err(e) => panic!("first write must succeed: {e}"),
    }
    assert_eq!(std::fs::read_to_string(dir.join("briefs/alpha.md")).unwrap(), body);
    // The bucket holds exactly the brief — no archive, no tmp residue.
    let names: Vec<String> = std::fs::read_dir(dir.join("briefs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha.md".to_string()]);
}

#[test]
fn brief_superseding_write_archives_superseded_mtime() {
    let dir = tmp_root("brief-archive");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    let v1 = "# Brief alpha v1\n";
    let v2 = "# Brief alpha v2\n";
    match briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v1})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("write 1 must succeed: {e}"),
    }
    // Pin the superseded content's mtime to a known second: the
    // archive stamp must encode THAT mtime (the contract's rule), not
    // the supersession moment.
    set_mtime(&dir.join("briefs/alpha.md"), FIXED_EPOCH);
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v2}));
    match r {
        HandlerResult::Ok(v) => assert_eq!(
            v["archived_to"],
            json!("briefs/alpha-2026-01-01T000000Z.md"),
            "the archive stamp is the superseded content's mtime, rendered in UTC"
        ),
        HandlerResult::Err(e) => panic!("write 2 must succeed: {e}"),
    }
    // The rename (not a copy) preserves the superseded bytes.
    let archive = dir.join("briefs/alpha-2026-01-01T000000Z.md");
    assert_eq!(std::fs::read_to_string(&archive).unwrap(), v1);
    assert_eq!(std::fs::read_to_string(dir.join("briefs/alpha.md")).unwrap(), v2);
}

#[test]
fn brief_same_second_collision_refused_zero_side_effects() {
    let dir = tmp_root("brief-collide");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    let v1 = "# Brief alpha v1\n";
    let v2 = "# Brief alpha v2\n";
    match briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v1})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("write 1 must succeed: {e}"),
    }
    set_mtime(&dir.join("briefs/alpha.md"), FIXED_EPOCH);
    // A different archive already occupies the same stamp.
    let planted = "planted, different content\n";
    std::fs::write(dir.join("briefs/alpha-2026-01-01T000000Z.md"), planted).unwrap();
    let before = std::fs::read(dir.join("briefs/alpha.md")).unwrap();
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v2}));
    match r {
        HandlerResult::Err(msg) => assert!(!msg.is_empty(), "collision must refuse loudly"),
        HandlerResult::Ok(_) => panic!("two supersessions sharing one mtime second must not overwrite the existing archive"),
    }
    assert_eq!(std::fs::read(dir.join("briefs/alpha.md")).unwrap(), before, "the current brief is untouched");
    assert_eq!(
        std::fs::read_to_string(dir.join("briefs/alpha-2026-01-01T000000Z.md")).unwrap(),
        planted,
        "the existing archive is never overwritten"
    );
    let leftovers: Vec<String> = std::fs::read_dir(dir.join("briefs"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".alpha.md.tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no tmp residue: {leftovers:?}");
}

#[test]
fn brief_idempotent_rearrival_keeps_archive() {
    let dir = tmp_root("brief-idem");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    let v1 = "# Brief alpha v1\n";
    match briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v1})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(e) => panic!("write 1 must succeed: {e}"),
    }
    set_mtime(&dir.join("briefs/alpha.md"), FIXED_EPOCH);
    // The same supersession already archived: the archive at the
    // matching stamp is byte-identical to the content being
    // superseded.
    std::fs::write(dir.join("briefs/alpha-2026-01-01T000000Z.md"), v1).unwrap();
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":v1}));
    match r {
        HandlerResult::Ok(v) => assert_eq!(
            v["archived_to"],
            json!("briefs/alpha-2026-01-01T000000Z.md"),
            "the existing archive is kept and re-reported"
        ),
        HandlerResult::Err(e) => panic!("a byte-identical re-arrival is idempotent, not a collision: {e}"),
    }
    assert_eq!(
        std::fs::read_to_string(dir.join("briefs/alpha-2026-01-01T000000Z.md")).unwrap(),
        v1,
        "the existing archive is not rewritten"
    );
    assert_eq!(std::fs::read_to_string(dir.join("briefs/alpha.md")).unwrap(), v1);
}

#[test]
fn brief_unsafe_worker_refused_no_bucket() {
    let dir = tmp_root("brief-unsafe");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    for w in ["", "..", "../escape", "sub/dir", "/abs/path"] {
        let r = briefs::write_brief(&mut s, "probe", &json!({"worker":w,"brief":"body"}));
        match r {
            HandlerResult::Err(msg) => assert!(!msg.is_empty(), "unsafe segment {w:?} must be refused"),
            HandlerResult::Ok(_) => panic!("unsafe segment {w:?} must be refused"),
        }
    }
    assert!(!dir.join("briefs").exists(), "worker safety runs before the bucket is created");
}

#[test]
fn brief_bad_args_refused_no_bucket() {
    let dir = tmp_root("brief-badargs");
    let mut s = seeded(&dir);
    full_claim(&dir, "probe", "t-brief", &mut s);
    for args in [
        json!({"worker":"alpha"}),
        json!({"worker":"alpha","brief":"x","extra":1}),
        json!({"worker":3,"brief":"x"}),
        json!("not-an-object"),
    ] {
        let r = briefs::write_brief(&mut s, "probe", &args);
        match r {
            HandlerResult::Err(msg) => assert!(!msg.is_empty(), "bad args {args} must be refused"),
            HandlerResult::Ok(_) => panic!("bad args {args} must be refused"),
        }
    }
    assert!(!dir.join("briefs").exists(), "argument validation runs before the bucket is created");
}
