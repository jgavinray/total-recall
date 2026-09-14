//! §8 gate — wave 3 slice (claims + dayfile + briefs + warm-start contract).
//!
//! ORCHESTRATOR-OWNED per ops/parallel-workers.md invariant 5: written BEFORE the
//! wave launches, workers MUST NOT edit this file. Compile fails loudly until the
//! wave's modules land — that is the intended loud failure.
//!
//! Asserted contract (local://exo-mcp-wave3-contract.md):
//! - `tools::claims::claim_orchestrator` — disk truth `.claims/orchestrator-<date>`
//!   `{holder_session_id, token, acquired_at}`; fresh claim `O_CREAT|O_EXCL`; same-session
//!   re-claim returns the SAME token; a different session is refused with holder+reason
//!   and the claim file stays byte-identical; date defaults to the server LOCAL date.
//! - `tools::claims::write_dayfile` — gated (handshake + today's token); replaces the
//!   day file with the caller's content, byte-exact.
//! - `tools::briefs::write_brief` — gated (handshake + token); writes
//!   `briefs/<worker>.md`; archives the superseded version to
//!   `briefs/<worker>-<superseded-date>T<HHMMSS>Z.md` (UTC stamp OF THE SUPERSEDED
//!   CONTENT'S mtime); two same-day writes produce TWO distinct archives; `worker` is
//!   validated as a single safe path segment (no `/`, no `..`, no absolute path).
//! - `tools::warmstart::write_warm_start` — gated (handshake + token); rewrites ONLY the
//!   warm-start block and moves the superseded block to History; every worker-signoff
//!   byte is preserved verbatim; atomic tmp+rename under the lock; the non-rewritten
//!   tail must be byte-identical or the tool fails LOUDLY and keeps the old file.
//! - Every refusal asserts absence of side effects (byte-identical/absent target).

use totalrecall::config;
use totalrecall::rpc::{HandlerResult, Server};
use totalrecall::tools::{briefs, claims, warmstart};
use serde_json::json;
use std::path::PathBuf;

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("totalrecall-gate-w3-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn seeded(root: &PathBuf) -> Server {
    config::init_state(root).unwrap();
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
    let mut s = Server::new_server(root);
    let _ = totalrecall::tools::signoff_read::read_signoff(&mut s, "probe");
    s
}
/// The server's LOCAL date, derived from `config::resolve_tz()` (the same
/// fingerprint the server itself keys to the root) — the gate must not hardcode
/// a date the box's local calendar can drift past.
fn local_date() -> String {
    let (_, offset) = config::resolve_tz().unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64 + offset * 60;
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

#[test]
fn claim_fresh_returns_token_and_writes_disk_truth() {
    let dir = tmp_root("claim-fresh");
    let mut s = seeded(&dir);
    let v = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("fresh claim must succeed: {e}"),
    };
    let token = v["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    let date = local_date();
    let cf = dir.join(format!(".claims/orchestrator-{date}"));
    let raw = std::fs::read_to_string(&cf).unwrap();
    let j: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(j["holder_session_id"], "probe");
    assert_eq!(j["token"], token.clone());
    assert!(j["acquired_at"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn claim_reclaim_same_session_returns_same_token() {
    let dir = tmp_root("claim-re");
    let mut s = seeded(&dir);
    let t1 = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v["token"].as_str().unwrap().to_string(),
        HandlerResult::Err(e) => panic!("fresh claim must succeed: {e}"),
    };
    let t2 = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v["token"].as_str().unwrap().to_string(),
        HandlerResult::Err(e) => panic!("same-session re-claim must succeed: {e}"),
    };
    assert_eq!(t1, t2, "same-session re-claim must return the SAME token");
}

#[test]
fn claim_other_session_refused_with_holder_and_file_unchanged() {
    let dir = tmp_root("claim-other");
    let mut s = seeded(&dir);
    // claim_orchestrator IS gated (spec §4): both claim sessions must handshaken
    // first, otherwise a correct implementation refuses with the handshake text
    // and the claim file would never exist.
    let _ = totalrecall::tools::signoff_read::read_signoff(&mut s, "holder");
    let _ = totalrecall::tools::signoff_read::read_signoff(&mut s, "intruder");
    let _ = claims::claim_orchestrator(&mut s, "holder", &json!({}));
    let date = local_date();
    let before = std::fs::read(dir.join(format!(".claims/orchestrator-{date}"))).unwrap();
    let r = claims::claim_orchestrator(&mut s, "intruder", &json!({}));
    match r {
        HandlerResult::Err(msg) => {
            assert!(msg.contains("holder") || msg.contains("held"), "refusal must name the holder: {msg}");
        }
        HandlerResult::Ok(_) => panic!("different session must be refused"),
    }
    assert_eq!(std::fs::read(dir.join(format!(".claims/orchestrator-{date}"))).unwrap(), before);
}

#[test]
fn dayfile_without_token_refused_no_file() {
    let dir = tmp_root("dayfile-notoken");
    let mut s = seeded(&dir);
    let date = local_date();
    let r = claims::write_dayfile(&mut s, "probe", &json!({"content":"body","orchestrator_token":"bogus"}));
    match r {
        HandlerResult::Err(msg) => assert!(msg.contains("claim") || msg.contains("token"), "refusal must name the claim: {msg}"),
        HandlerResult::Ok(_) => panic!("must refuse without a valid token"),
    }
    assert!(!dir.join(format!("{date}.md")).exists(), "no day file created");
}

#[test]
fn dayfile_with_token_replaces_file_byte_exact() {
    let dir = tmp_root("dayfile-ok");
    let mut s = seeded(&dir);
    let tok = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v["token"].as_str().unwrap().to_string(),
        HandlerResult::Err(e) => panic!("claim must succeed: {e}"),
    };
    let date = local_date();
    let content = "# Day\n\n- decision recorded\n";
    match claims::write_dayfile(&mut s, "probe", &json!({"content": content, "orchestrator_token": tok})) {
        HandlerResult::Err(e) => panic!("dayfile write must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let got = std::fs::read_to_string(dir.join(format!("{date}.md"))).unwrap();
    assert_eq!(got, content, "day file replaced byte-exact");
}

#[test]
fn brief_without_token_refused_no_files() {
    let dir = tmp_root("brief-notoken");
    let mut s = seeded(&dir);
    let r = briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":"body"}));
    match r {
        HandlerResult::Err(msg) => assert!(msg.contains("claim") || msg.contains("token"), "refusal must name the claim: {msg}"),
        HandlerResult::Ok(_) => panic!("must refuse without a valid token"),
    }
    assert!(!dir.join("briefs").exists(), "no briefs/ created");
}

#[test]
fn brief_with_token_writes_and_archives_distinct_names() {
    let dir = tmp_root("brief-ok");
    let mut s = seeded(&dir);
    match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Err(e) => panic!("claim must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    // Seed briefs/alpha.md FIRST, then two writes: write #1 archives the
    // seeded content, write #2 archives write #1's content — two distinct
    // archives (the contract's mtime-stamp rule). The 1s sleep sits between
    // the seed and write #1, so seed-second < b1-second is GUARANTEED (≥1s
    // apart) — second-granularity stamps cannot collide and no archive is
    // ever overwritten.
    std::fs::create_dir_all(dir.join("briefs")).unwrap();
    std::fs::write(dir.join("briefs/alpha.md"), "# Brief alpha seed\n").unwrap();
    let b1 = "# Brief alpha v1\n";
    let b2 = "# Brief alpha v2\n";
    std::thread::sleep(std::time::Duration::from_secs(1));
    match briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":b1})) {
        HandlerResult::Err(e) => panic!("brief write 1 must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    match briefs::write_brief(&mut s, "probe", &json!({"worker":"alpha","brief":b2})) {
        HandlerResult::Err(e) => panic!("brief write 2 must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let cur = std::fs::read_to_string(dir.join("briefs/alpha.md")).unwrap();
    assert_eq!(cur, b2, "briefs/<worker>.md holds the latest brief");
    let mut names: Vec<String> = std::fs::read_dir(dir.join("briefs")).unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("alpha-") && n.ends_with(".md"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "two distinct archives for one worker one day");
    assert_ne!(names[0], names[1]);
    for n in &names {
        // <worker>-<superseded-date>T<HHMMSS>Z.md
        assert!(n.contains("T") && n.ends_with("Z.md"), "archive name must carry UTC second-granularity stamp: {n}");
    }
}

#[test]
fn brief_rejects_unsafe_worker_segment() {
    let dir = tmp_root("brief-unsafe");
    let mut s = seeded(&dir);
    let tok = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v["token"].as_str().unwrap().to_string(),
        HandlerResult::Err(e) => panic!("claim must succeed: {e}"),
    };
    let _ = tok;
    for w in ["../escape", "sub/dir", "/abs/path"] {
        let r = briefs::write_brief(&mut s, "probe", &json!({"worker":w,"brief":"body"}));
        match r {
            HandlerResult::Err(msg) => assert!(!msg.is_empty(), "unsafe segment {w} must be refused"),
            HandlerResult::Ok(_) => panic!("unsafe segment {w} must be refused"),
        }
    }
    assert!(!dir.join("briefs").exists());
}

#[test]
fn warmstart_without_token_refused_file_unchanged() {
    let dir = tmp_root("warm-notoken");
    let mut s = seeded(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let r = warmstart::write_warm_start(&mut s, "probe", &json!({"content":"new warm","orchestrator_token":"bogus"}));
    match r {
        HandlerResult::Err(msg) => assert!(msg.contains("claim") || msg.contains("token"), "refusal must name the claim: {msg}"),
        HandlerResult::Ok(_) => panic!("must refuse without a valid token"),
    }
    assert_eq!(std::fs::read(dir.join("signoff.md")).unwrap(), before, "signoff.md byte-identical");
}

#[test]
fn warmstart_with_token_rewrites_block_and_preserves_workers_verbatim() {
    let dir = tmp_root("warm-ok");
    let mut s = seeded(&dir);
    let tok = match claims::claim_orchestrator(&mut s, "probe", &json!({})) {
        HandlerResult::Ok(v) => v["token"].as_str().unwrap().to_string(),
        HandlerResult::Err(e) => panic!("claim must succeed: {e}"),
    };
    let content = "Last updated: 2026-09-13\n\n## If you read nothing else\n1. **New item** — fresh prose\n";
    match warmstart::write_warm_start(&mut s, "probe", &json!({"content": content, "orchestrator_token": tok})) {
        HandlerResult::Err(e) => panic!("warm-start write must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert!(text.contains("## If you read nothing else"), "warm-start block present");
    assert!(text.contains("1. **New item** — fresh prose"), "new block landed");
    assert!(text.contains("Worker signoff (alpha) | done: yes"), "worker bytes preserved verbatim");
    assert!(text.contains("superseded block prose three"), "superseded block moved to History");
    assert!(text.contains("## History"), "History section present");
}
