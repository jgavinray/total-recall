//! §8 gate — wave 2 slice (handshake gate + read_signoff + append_signoff contract).
//!
//! ORCHESTRATOR-OWNED per ops/parallel-workers.md invariant 5: written BEFORE the
//! wave launches, workers MUST NOT edit this file. Compile fails loudly until the
//! wave's modules land — that is the intended loud failure.
//!
//! Asserted contract (local://exo-mcp-build-contract.md §wave-2):
//! - `gate::handshake(server, session)` marks the session handshaken; `gate::gate(server, session)`
//!   refuses every gated tool until then, and records the refused attempt as
//!   `attempted_write_before_handshake`.
//! - `tools::signoff::read_signoff` parses the seeded signoff.md: warm-start block
//!   (`## If you read nothing else` … next `## ` heading), ranked numbered lines, History,
//!   worker-signoff lines.
//! - `tools::signoff::append_signoff` emits EXACTLY ONE line
//!   `Worker signoff (<role>) | done: <yes|no> | unpushed: … | awaits human: … | still running: …`
//!   (+ optional `| kaibo review: …` then `| workflow: … | ts: <ISO-8601Z> | session: …`),
//!   under an `O_CREAT|O_EXCL` lock file in `.locks/`; >16384 B refused with ZERO bytes written.
//! - Two racing appends (one process) and two server processes on one root both serialize.

use exomem_mcp::config::{self, ConfigArgs};
use exomem_mcp::gate;
use exomem_mcp::rpc::{self, HandlerResult, Server};
use exomem_mcp::tools::signoff;
use serde_json::json;
use std::path::PathBuf;

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-gate-w2-{name}-{}", std::process::id()));
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

## History
- 2026-09-12 — superseded block prose three
"#;

fn seed(root: &PathBuf) {
    std::fs::write(root.join("signoff.md"), SEED).unwrap();
}

#[test]
fn read_signoff_parses_warm_start_ranked_history_and_workers() {
    let dir = tmp_root("read");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    let session = "probe";
    let v = match signoff::read_signoff(&mut s, session) {
        HandlerResult::Ok(v) => v,
        HandlerResult::Err(e) => panic!("read_signoff must succeed on a seeded fixture: {e}"),
    };
    assert!(v["warm_start"]["header"].as_str().unwrap().contains("Last updated:"));
    let ranked = v["warm_start"]["ranked"].as_array().unwrap();
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0]["rank"], 1);
    assert!(ranked[0]["text"].as_str().unwrap().contains("First item"));
    assert!(ranked[1]["text"].as_str().unwrap().contains("Second item"));
    let history = v["warm_start"]["history"].as_array().unwrap();
    assert!(!history.is_empty());
    assert!(history.iter().any(|h| h.as_str().unwrap().contains("superseded")));
    let ws = v["worker_signoffs"].as_array().unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0]["worker"], "alpha");
    assert_eq!(ws[0]["done"], "yes");
}

#[test]
fn handshake_marks_session_and_persists_id() {
    let dir = tmp_root("hand");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    assert!(!gate::gate(&mut s, "probe").is_ok(), "gate must refuse before handshake");
    let _ = signoff::read_signoff(&mut s, "probe");
    assert!(gate::gate(&mut s, "probe").is_ok(), "gate must pass after handshake");
    let sess = dir.join(".state/sessions.json");
    assert!(sess.exists(), ".state/sessions.json must persist the session id");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&sess).unwrap()).unwrap();
    assert!(v.is_array());
    assert!(v.as_array().unwrap().iter().any(|e| e["session_id"] == "probe"));
}

#[test]
fn append_before_handshake_is_refused_and_writes_nothing() {
    let dir = tmp_root("append-noshake");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let mut s = Server::new_server(&dir);
    let args = json!({"role":"probe","workflow":"memory-kernel","done":"yes","unpushed":"n/a (no repo writes)","awaits_human":"none","still_running":"no","kaibo_review":"n/a (no code changes)"});
    let r = signoff::append_signoff(&mut s, "probe", &args);
    match r {
        HandlerResult::Err(msg) => assert!(
            msg.contains("handshake incomplete"),
            "refusal must restate the rule: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("must refuse before handshake"),
    }
    assert_eq!(
        std::fs::read(dir.join("signoff.md")).unwrap(),
        before,
        "side effect ABSENT: file byte-identical"
    );
    let audit = dir.join(".audit");
    assert!(!audit.exists(), "no audit log should exist before handshake");
}

#[test]
fn append_after_handshake_emits_legacy_line_with_stamps() {
    let dir = tmp_root("append-ok");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    let _ = signoff::read_signoff(&mut s, "probe");
    let args = json!({"role":"probe","workflow":"memory-kernel","done":"yes","unpushed":"n/a (no repo writes)","awaits_human":"none","still_running":"no","kaibo_review":"n/a (no code changes)"});
    match signoff::append_signoff(&mut s, "probe", &args) {
        HandlerResult::Err(e) => panic!("append must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    let appended = text
        .lines()
        .filter(|l| l.starts_with("Worker signoff (probe)"))
        .count();
    assert_eq!(appended, 1, "exactly ONE appended line");
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .unwrap();
    assert!(line.contains("| done: yes"));
    assert!(line.contains("| unpushed: n/a (no repo writes)"));
    assert!(line.contains("| awaits human: none"));
    assert!(line.contains("| still running: no"));
    assert!(line.contains("| kaibo review: n/a (no code changes)"));
    assert!(line.contains("| workflow: memory-kernel"));
    let ts = line.split("| ts: ").nth(1).unwrap().trim().to_string();
    assert!(ts.ends_with('Z') && ts.len() >= 20, "server-stamped ts must be ISO-8601Z: {ts}");
    assert!(line.contains("| session: "));
    // pre-existing bytes preserved verbatim
    assert!(text.contains("Worker signoff (alpha) | done: yes"));
}

#[test]
fn append_cap_15000_passes_and_20000_refused_zero_bytes() {
    let dir = tmp_root("cap");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    let _ = signoff::read_signoff(&mut s, "probe");
    let big15000 = "x".repeat(15000);
    let args = json!({"role":"probe","workflow":"memory-kernel","done":"yes","unpushed":big15000,"awaits_human":"none","still_running":"no"});
    match signoff::append_signoff(&mut s, "probe", &args) {
        HandlerResult::Err(e) => panic!("15000-byte entry must PASS (lock carries size): {e}"),
        HandlerResult::Ok(_) => {}
    }
    let big20000 = "y".repeat(20000);
    let args2 = json!({"role":"probe","workflow":"memory-kernel","done":"yes","unpushed":big20000,"awaits_human":"none","still_running":"no"});
    let before = std::fs::read(dir.join("signoff.md")).unwrap().len();
    match signoff::append_signoff(&mut s, "probe", &args2) {
        HandlerResult::Err(msg) => assert!(msg.contains("16384") || msg.contains("16384-byte"), "cap refusal must name the limit: {msg}"),
        HandlerResult::Ok(_) => panic!("20000-byte entry must be REFUSED"),
    }
    assert_eq!(
        std::fs::read(dir.join("signoff.md")).unwrap().len(),
        before,
        "side effect ABSENT: zero bytes written"
    );
}

#[test]
fn kaibo_review_is_passed_through_verbatim_never_synthesized() {
    let dir = tmp_root("kaibo");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    let _ = signoff::read_signoff(&mut s, "probe");
    let args = json!({"role":"probe","workflow":"memory-kernel","done":"yes","unpushed":"none","awaits_human":"none","still_running":"no","kaibo_review":"job-42 (vllm-local) @ 2026-09-13"});
    match signoff::append_signoff(&mut s, "probe", &args) {
        HandlerResult::Err(e) => panic!("append must succeed: {e}"),
        HandlerResult::Ok(_) => {}
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .unwrap();
    assert!(line.contains("| kaibo review: job-42 (vllm-local) @ 2026-09-13"));
}

#[test]
fn racing_appends_serialize_no_interleave() {
    let dir = tmp_root("race");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let mut s = Server::new_server(&dir);
    let _ = signoff::read_signoff(&mut s, "probe");
    let mut handles = Vec::new();
    for i in 0..10 {
        let root = dir.clone();
        let role = format!("probe{i}");
        let wf = format!("wf-{i}");
        handles.push(std::thread::spawn(move || {
            let mut s2 = Server::new_server(&root);
            let _ = signoff::read_signoff(&mut s2, &role);
            let args = json!({"role": role, "workflow": wf, "done":"yes", "unpushed":"none", "awaits_human":"none", "still_running":"no"});
            match signoff::append_signoff(&mut s2, &role, &args) {
                HandlerResult::Err(e) => Err(format!("{role}: {e}")),
                HandlerResult::Ok(_) => Ok(()),
            }
        }));
    }
    for h in handles {
        let r = h.join().unwrap();
        assert!(r.is_ok(), "every racing append must succeed: {:?}", r.err());
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    let mut ok = 0;
    for i in 0..10 {
        let role = format!("probe{i}");
        let want = format!("Worker signoff ({role}) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: wf-{i}");
        assert!(text.contains(&want), "entry {i} must be intact");
        ok += 1;
    }
    assert_eq!(ok, 10, "all 10 entries intact, no interleave");
    assert_eq!(
        text.lines().filter(|l| l.starts_with("Worker signoff (probe")).count(),
        10
    );
}

#[test]
fn two_server_processes_on_one_root_serialize() {
    let dir = tmp_root("twoproc");
    config::init_state(&dir).unwrap();
    seed(&dir);
    let bin = std::env::var("CARGO_BIN_EXE_exomem-mcp").expect("cargo test builds the bin target");
    let frames_a = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{{\"protocolVersion\":\"2026-07-28\",\"capabilities\":{{}},\"clientInfo\":{{\"name\":\"a\",\"version\":\"0\"}}}}}}\n{{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{{\"name\":\"read_signoff\",\"arguments\":{{}}}}}}\n{{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{{\"name\":\"append_signoff\",\"arguments\":{{\"role\":\"procA\",\"workflow\":\"wf-a\",\"done\":\"yes\",\"unpushed\":\"none\",\"awaits_human\":\"none\",\"still_running\":\"no\"}}}}}}\n"
    );
    let frames_b = frames_a.replace("\"procA\"", "\"procB\"").replace("wf-a", "wf-b");
    let mut children = Vec::new();
    for (frames, tag) in [(frames_a.clone(), "a"), (frames_b.clone(), "b")] {
        let mut c = std::process::Command::new(&bin)
            .arg("--root")
            .arg(dir.display().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        use std::io::Write;
        let mut si = c.stdin.take().unwrap();
        si.write_all(frames.as_bytes()).unwrap();
        drop(si);
        children.push((c, tag));
    }
    for (mut c, tag) in children {
        let out = c.wait_with_output().unwrap();
        assert!(out.status.success(), "process {tag} must exit 0");
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert!(text.contains("Worker signoff (procA) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: wf-a"));
    assert!(text.contains("Worker signoff (procB) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: wf-b"));
    assert!(text.contains("Worker signoff (alpha) | done: yes"), "pre-existing bytes intact");
}
