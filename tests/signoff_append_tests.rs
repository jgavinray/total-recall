//! Self-checks for `tools::signoff_append` (wave 2).
//!
//! Sibling-coupling note (wave-2 contract): these tests do NOT depend on
//! `gate.rs` — instead of going through `gate::handshake` /
//! `signoff_read::read_signoff` they set `Server.handshaken` directly
//! (a public field), so this file compiles and runs even before the
//! sibling module lands. The full-stack contract — including the gate
//! through `read_signoff`, the cross-process race, and the `signoff.md`
//! stamp format — is asserted by the orchestrator-owned `tests/gate_w2.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use totalrecall::rpc::{HandlerResult, Server};
use totalrecall::tools::signoff_append;
use serde_json::json;

/// The on-disk format is a shipped reference (the real `~/dev/exomemory/
/// signoff.md`); fixture mirrors its layout.
const SEED: &str = "# Signoff

Last updated: 2026-09-13 (08:20 PDT) · by this session

## If you read nothing else
1. **First item** — prose one
2. **Second item** — prose two

## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a

## History
- 2026-09-12 — superseded block
";

fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "totalrecall-w2append-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A root seeded with the reference signoff.md fixture.
fn seeded_root(name: &str) -> PathBuf {
    let dir = tmp_root(name);
    std::fs::write(dir.join("signoff.md"), SEED).unwrap();
    dir
}

/// A handshaken server on `dir` for the given session (the direct-API
/// analogue of `read_signoff` — public field, no sibling dependency).
fn handshaken_server(dir: &std::path::Path, session: &str) -> Server {
    let mut s = Server::new_server(dir);
    s.handshaken.insert(session.to_string(), true);
    s
}

/// The full six-field argument set the spec §3 example pins.
fn full_args() -> serde_json::Value {
    json!({
        "role": "probe",
        "workflow": "memory-kernel",
        "done": "yes",
        "unpushed": "n/a (no repo writes)",
        "awaits_human": "none",
        "still_running": "no",
        "kaibo_review": "n/a (no code changes)",
    })
}

/// Split the server-stamped tail of an entry: everything after
/// `| ts: ` is `<ts> | session: <session-id>`.
fn split_ts_session(line: &str) -> (String, String) {
    let rest = line
        .split("| ts: ")
        .nth(1)
        .expect("entry must carry the server-stamped | ts: ");
    let mut it = rest.splitn(2, " | session: ");
    let ts = it.next().expect("ts present").to_string();
    let session = it.next().expect("session present").to_string();
    (ts, session)
}

/// spec §2: "all emitted timestamps are UTC with a trailing Z".
fn assert_iso8601z(ts: &str) {
    assert_eq!(ts.len(), 20, "ts is second-precision ISO-8601Z: {ts}");
    for (i, c) in ts.char_indices() {
        match i {
            4 | 7 => assert_eq!(c, '-', "date separators in {ts}"),
            10 => assert_eq!(c, 'T', "date/time separator in {ts}"),
            13 | 16 => assert_eq!(c, ':', "time separators in {ts}"),
            19 => assert_eq!(c, 'Z', "UTC designator in {ts}"),
            _ => assert!(c.is_ascii_digit(), "digits in {ts}"),
        }
    }
}

#[test]
fn append_before_handshake_refused_with_zero_side_effects() {
    let dir = seeded_root("no-shake");
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    let mut s = Server::new_server(&dir); // handshaken is empty

    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Err(msg) => {
            assert!(
                msg.contains("handshake incomplete"),
                "the exact pinned refusal text is restated: {msg}"
            );
        }
        HandlerResult::Ok(_) => panic!("must refuse before the handshake"),
    }
    // The refused attempt is counted, per-session (spec §4), so
    // session_compliance can observe it later.
    assert_eq!(
        s.attempted_write_before_handshake.get("probe").copied(),
        Some(1),
        "the refused pre-handshake attempt is counted"
    );
    // ZERO side effects: not a byte, not even the lock file.
    assert_eq!(
        std::fs::read(dir.join("signoff.md")).unwrap(),
        before,
        "a refused append writes nothing"
    );
    assert!(!dir.join(".locks").exists(), "no lock file is created for a refused call");
    assert!(!dir.join(".audit").exists(), "no audit trail for a refused call");
}

#[test]
fn append_after_handshake_emits_the_pinned_one_line() {
    let dir = seeded_root("one-line");
    let mut s = handshaken_server(&dir, "probe");

    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("handshaken append must succeed: {msg}"),
    }

    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    // Pre-existing bytes preserved verbatim: the seeded file is a byte
    // prefix of the result; the entry is appended after it.
    assert!(text.starts_with(SEED), "pre-existing bytes are a verbatim prefix");
    // EXACTLY one new line, in the pinned field order:
    // token, the four status fields, kaibo review, then the server stamps.
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .expect("the probe entry was appended");
    let (ts, session) = split_ts_session(line);
    assert_iso8601z(&ts);
    assert_eq!(
        session, s.session_id,
        "the session stamp is the process-local server session id"
    );
    let expected = format!(
        "Worker signoff (probe) | done: yes | unpushed: n/a (no repo writes) | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: {ts} | session: {}",
        s.session_id
    );
    assert_eq!(line, expected, "the serialized entry is exactly the pinned format");
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("Worker signoff (probe)"))
            .count(),
        1,
        "exactly ONE line was appended"
    );
    // The seeded alpha entry survived, byte for byte.
    assert!(text.contains("Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a"));
    // Release = the server removes its own lock file.
    assert!(
        !dir.join(".locks").join("signoff.md.lock").exists(),
        "the lock file was released (removed) after the append"
    );
}

#[test]
fn defaults_fill_optional_fields_and_kaibo_review_never_synthesized() {
    let dir = seeded_root("defaults");
    let mut s = handshaken_server(&dir, "probe");

    match signoff_append::append_signoff(&mut s, "probe", &json!({"role": "probe", "workflow": "wf-x", "done": "no"})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("handshaken append must succeed: {msg}"),
    }

    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .expect("entry appended");
    // Schema defaults: unpushed 'none', awaits human 'none', still
    // running 'no'; no kaibo review field was supplied, and the server
    // NEVER synthesizes one.
    let prefix = "Worker signoff (probe) | done: no | unpushed: none | awaits human: none | still running: no | workflow: wf-x | ts: ";
    assert!(line.starts_with(prefix), "schema defaults applied in pinned order: {line}");
    assert!(
        !line.contains("kaibo review"),
        "kaibo review is never synthesized by the server: {line}"
    );
}

#[test]
fn oversize_entry_refused_zero_bytes_and_16384_named() {
    let dir = seeded_root("cap");
    let mut s = handshaken_server(&dir, "probe");

    // 15000-byte unpushed: under the cap → appended.
    let ok_args = json!({"role": "probe", "workflow": "memory-kernel", "done": "yes", "unpushed": "x".repeat(15000)});
    let before = std::fs::read(dir.join("signoff.md")).unwrap();
    match signoff_append::append_signoff(&mut s, "probe", &ok_args) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("15000-byte entry must fit under the 16384 cap: {msg}"),
    }
    let after = std::fs::read(dir.join("signoff.md")).unwrap();
    assert!(
        after.len() > before.len(),
        "the 15000-byte entry was appended"
    );

    // 20000-byte unpushed: over the cap → refused, ZERO bytes written,
    // and the refusal message names the cap.
    let big_args = json!({"role": "probe", "workflow": "memory-kernel", "done": "yes", "unpushed": "x".repeat(20000)});
    match signoff_append::append_signoff(&mut s, "probe", &big_args) {
        HandlerResult::Err(msg) => {
            assert!(
                msg.contains("16384"),
                "the refusal names the 16384-byte cap: {msg}"
            );
        }
        HandlerResult::Ok(_) => panic!("a 20000-byte entry must be refused"),
    }
    assert_eq!(
        std::fs::read(dir.join("signoff.md")).unwrap(),
        after,
        "the refused oversize append wrote zero bytes"
    );
    assert!(
        !dir.join(".locks").join("signoff.md.lock").exists(),
        "the cap is enforced before the lock: no lock file remains"
    );
}

#[test]
fn kaibo_review_passed_through_verbatim() {
    let dir = seeded_root("kaibo");
    let mut s = handshaken_server(&dir, "probe");
    let args = json!({
        "role": "probe",
        "workflow": "memory-kernel",
        "done": "yes",
        "kaibo_review": "job-42 (vllm-local) @ 2026-09-13 — SHIP-WITH-CHANGES (see notes)",
    });
    match signoff_append::append_signoff(&mut s, "probe", &args) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("handshaken append must succeed: {msg}"),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .expect("entry appended");
    // Verbatim — parens, '@', em-dashes and all — in its pinned position
    // (after the four status fields, before the server stamps).
    assert!(
        line.contains("| kaibo review: job-42 (vllm-local) @ 2026-09-13 — SHIP-WITH-CHANGES (see notes) | workflow: "),
        "kaibo review is passed through verbatim in pinned position: {line}"
    );
}

#[test]
fn racing_appends_serialize_no_interleave() {
    let dir = Arc::new(seeded_root("race"));
    let mut handles = Vec::new();
    for i in 0..10u32 {
        let root = Arc::clone(&dir);
        let session = format!("w{i}");
        handles.push(thread::spawn(move || {
            let mut s2 = Server::new_server(&root);
            s2.handshaken.insert(session.clone(), true);
            let args = json!({
                "role": session,
                "workflow": format!("wf-{i}"),
                "done": "yes",
                "unpushed": "none",
                "awaits_human": "none",
                "still_running": "no"
            });
            let r = signoff_append::append_signoff(&mut s2, &session, &args);
            (r, s2.session_id.clone())
        }));
    }
    for h in handles {
        let (r, sid) = h.join().unwrap();
        match r {
            HandlerResult::Ok(_) => {}
            HandlerResult::Err(msg) => panic!("a racing append must not fail: {msg}"),
        }
        let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
        // No interleaving: the entry's line is intact (prefix + stamps)
        // and stamped with THIS process's session id.
        let line = text
            .lines()
            .find(|l| l.ends_with(&format!(" | session: {sid}")))
            .unwrap_or_else(|| panic!("entry stamped with this session is missing:\n{text}"));
        assert!(
            line.starts_with("Worker signoff (w"),
            "the racing entry is intact and not interleaved: {line}"
        );
        assert_eq!(
            text.lines()
                .filter(|l| l.ends_with(&format!(" | session: {sid}")))
                .count(),
            1,
            "exactly one line is stamped with this session"
        );
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    // All 10 entries landed; the seeded alpha entry survived.
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("Worker signoff ("))
            .count(),
        11,
        "seed + 10 racing entries, no loss and no duplication"
    );
    for i in 0..10u32 {
        assert!(
            text.lines().any(|l| l.starts_with(&format!(
                "Worker signoff (w{i}) | done: yes | unpushed: none | awaits human: none | still running: no | workflow: wf-{i} | ts: "
            ))),
            "all 10 racing entries are intact: {text}"
        );
    }
    // Every lock was released.
    assert!(
        !dir.join(".locks").join("signoff.md.lock").exists(),
        "all locks were released"
    );
}

#[test]
fn fresh_lock_held_refuses_after_bounded_retry() {
    let dir = seeded_root("fresh-lock");
    let mut s = handshaken_server(&dir, "probe");
    let before = std::fs::read(dir.join("signoff.md")).unwrap();

    // A FRESH lock held by a live (simulated) holder: pid/session of a
    // holder that is NOT this process, epoch = now.
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let locks_dir = dir.join(".locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    let planted = format!(
        "pid={} nonce=12345 epoch={epoch} session=s-somebodyelse\n",
        std::process::id() + 1
    );
    std::fs::write(locks_dir.join("signoff.md.lock"), &planted).unwrap();

    let started = Instant::now();
    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Err(msg) => {
            assert!(
                msg.contains("bounded retry"),
                "a fresh held lock ends in the bounded-retry refusal: {msg}"
            );
        }
        HandlerResult::Ok(_) => panic!("a fresh held lock must NOT be taken over"),
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(4500),
        "the retry budget (50 x 100 ms) was actually consumed, not short-circuited: {elapsed:?}"
    );
    assert_eq!(
        std::fs::read(dir.join("signoff.md")).unwrap(),
        before,
        "zero bytes written"
    );
    // We did not touch the live holder's lock file.
    assert_eq!(
        std::fs::read_to_string(locks_dir.join("signoff.md.lock")).unwrap(),
        planted,
        "a fresh lock is never removed"
    );
}

#[test]
fn stale_lock_taken_over_server_side() {
    let dir = seeded_root("stale-lock");
    let mut s = handshaken_server(&dir, "probe");

    // A STALE lock: its holder has been silent for 40 s (> the 30 s
    // staleness threshold) — the server detects it by age and takes it
    // over, without spending the wait budget.
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let locks_dir = dir.join(".locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    let planted = format!(
        "pid=999999 nonce=7 epoch={} session=s-deadbeef\n",
        epoch - 40
    );
    std::fs::write(locks_dir.join("signoff.md.lock"), &planted).unwrap();

    let started = Instant::now();
    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("a stale lock must be taken over, not waited out: {msg}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the takeover path does not consume the 5 s wait budget"
    );
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert!(
        text.lines().any(|l| l.starts_with("Worker signoff (probe)")),
        "the append landed under the taken-over lock"
    );
    assert!(
        !locks_dir.join("signoff.md.lock").exists(),
        "release removed the (re-acquired) lock file"
    );
}

/// Schema violations and one-line-invariant violations are all refused
/// with ZERO side effects. Each case: handshaken server (so the gate
/// doesn't mask the validation refusal), exact-ish naming in the
/// message, no bytes, no lock.
#[test]
fn validation_refusals_write_nothing() {
    let cases: &[(&str, serde_json::Value, &str)] = &[
        ("missing-role", json!({"workflow": "wf", "done": "yes"}), "role"),
        (
            "missing-workflow",
            json!({"role": "probe", "done": "yes"}),
            "workflow",
        ),
        ("missing-done", json!({"role": "probe", "workflow": "wf"}), "done"),
        (
            "done-outside-enum",
            json!({"role": "probe", "workflow": "wf", "done": "maybe"}),
            "done",
        ),
        (
            "done-not-string",
            json!({"role": "probe", "workflow": "wf", "done": 1}),
            "string",
        ),
        (
            "unknown-field",
            json!({"role": "probe", "workflow": "wf", "done": "yes", "rogue": "x"}),
            "rogue",
        ),
        (
            "newline-in-field",
            json!({"role": "probe", "workflow": "wf", "done": "yes", "unpushed": "a\nb"}),
            "newline",
        ),
        (
            "empty-role",
            json!({"role": "", "workflow": "wf", "done": "yes"}),
            "role",
        ),
        (
            "role-with-parens",
            json!({"role": "a) b", "workflow": "wf", "done": "yes"}),
            "role",
        ),
        (
            "non-object-args",
            json!([1, 2]),
            "object",
        ),
    ];
    for (name, args, needle) in cases {
        let dir = seeded_root(name);
        let before = std::fs::read(dir.join("signoff.md")).unwrap();
        let mut s = handshaken_server(&dir, "probe");
        match signoff_append::append_signoff(&mut s, "probe", args) {
            HandlerResult::Err(msg) => {
                assert!(
                    msg.contains(needle),
                    "[{name}] the refusal names the problem ({needle:?}): {msg}"
                );
            }
            HandlerResult::Ok(_) => panic!("[{name}] must be refused"),
        }
        assert_eq!(
            std::fs::read(dir.join("signoff.md")).unwrap(),
            before,
            "[{name}] zero bytes written"
        );
        assert!(
            !dir.join(".locks").exists(),
            "[{name}] no lock file for a validation refusal"
        );
    }
}

#[test]
fn fresh_root_signoff_created_on_first_write() {
    let dir = tmp_root("fresh-root"); // no signoff.md at all
    let mut s = handshaken_server(&dir, "probe");

    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("first write on a fresh root must create signoff.md: {msg}"),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    assert!(
        text.starts_with("# Signoff"),
        "created-on-first-write scaffolds the file header"
    );
    assert!(
        text.contains("## Worker sessions — sign off here as you go"),
        "the entry lands under its section"
    );
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .expect("the entry is in the created file");
    let (ts, session) = split_ts_session(line);
    assert_iso8601z(&ts);
    assert_eq!(session, s.session_id);
    assert!(
        text.lines()
            .filter(|l| l.starts_with("Worker signoff ("))
            .count()
            == 1
    );
}

#[test]
fn missing_trailing_newline_gets_a_separator_inserted() {
    let dir = tmp_root("no-trailing-nl");
    // Pre-existing content that does NOT end with a newline.
    let partial = "Worker signoff (alpha) | done: yes";
    std::fs::write(dir.join("signoff.md"), partial).unwrap();
    let mut s = handshaken_server(&dir, "probe");

    match signoff_append::append_signoff(&mut s, "probe", &json!({"role": "probe", "workflow": "wf-x", "done": "no"})) {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("append must succeed: {msg}"),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).unwrap();
    // Pre-existing bytes verbatim; one separator inserted so the entry
    // is its own line.
    assert!(
        text.starts_with(&format!("{partial}\n")),
        "pre-existing bytes are preserved and a separator was inserted"
    );
    let probe_line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (probe)"))
        .expect("the entry is a line of its own");
    let (ts, session) = split_ts_session(probe_line);
    assert_iso8601z(&ts);
    assert_eq!(session, s.session_id);
    assert_eq!(
        text.lines().count(),
        2,
        "exactly two lines: the pre-existing one and the new entry"
    );
}

#[test]
fn tool_shape_is_spec_verbatim_and_dispatch_is_wired() {
    // tools(): one tool, the pinned name, the MANDATORY contract text in
    // the description (and no wave-1 placeholder suffix), the pinned
    // schema shape.
    let tools = signoff_append::tools();
    assert_eq!(tools.len(), 1, "exactly one tool");
    let tool = &tools[0];
    assert_eq!(tool.name, "append_signoff");
    assert!(
        tool.description.contains("handshake incomplete — call read_signoff first"),
        "the description carries the exact gate text"
    );
    assert!(
        !tool.description.contains("[wave 1:"),
        "the wave-1 placeholder suffix is gone: the body lands in wave 2"
    );
    assert_eq!(
        tool.input_schema["required"],
        json!(["role", "workflow", "done"])
    );
    assert_eq!(tool.input_schema["additionalProperties"], json!(false));
    assert_eq!(tool.input_schema["properties"]["done"]["enum"], json!(["yes", "no"]));
    assert!(tool.input_schema["properties"]["kaibo_review"].is_object());

    // handlers(): the dispatch entry the orchestrator-owned registry
    // merges — in the live server the session IS server.session_id.
    let dir2 = Arc::new(seeded_root("tooling-dispatch"));
    let mut s = Server::new_server(&dir2);
    s.handshaken.insert(s.session_id.clone(), true);
    let handlers = signoff_append::handlers();
    let h = *handlers
        .get("append_signoff")
        .expect("the dispatch entry is registered under the tool name");
    let r = h(&mut s, &json!({"role": "dispatch", "workflow": "wf-d", "done": "yes"}));
    match r {
        HandlerResult::Ok(_) => {}
        HandlerResult::Err(msg) => panic!("the wired handler must succeed: {msg}"),
    }
    let text = std::fs::read_to_string(dir2.join("signoff.md")).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("Worker signoff (dispatch)"))
        .expect("the dispatch entry appended");
    assert!(
        line.ends_with(&format!(" | session: {}", s.session_id)),
        "the handler stamps the process-local session id"
    );
}

/// F1 (signoffs/review_Wave4OKF.md 2d) — the review's MAJOR: an audit
/// tail that cannot land used to render as `isError` while the signoff
/// line sat on disk, the exact both-halves FAIL spec §8 declares. The
/// write HAS landed, so the result must be a SUCCESS carrying the
/// missing tail as `audit_error`. Pre-fix, this call returned
/// `HandlerResult::Err` and the Ok-arm panic below fired.
///
/// The audit path is made to fail deterministically on this box:
/// `.audit` is planted as a PLAIN FILE, so `append_audit`'s
/// `create_dir_all(.audit)` can never produce the jsonl.
#[test]
fn audit_tail_failure_lands_the_line_and_reports_audit_error() {
    let dir = seeded_root("f1-audit-tail");
    std::fs::write(dir.join(".audit"), "not a directory — the audit tail must fail to land").unwrap();
    let mut s = handshaken_server(&dir, "probe");

    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Ok(v) => {
            assert_eq!(v["appended"], json!(true), "the landed write reports success: {v}");
            assert!(
                v["audit_error"].as_str().is_some_and(|e| !e.is_empty()),
                "the missing tail is reported on the success result: {v}"
            );
        }
        HandlerResult::Err(msg) => panic!(
            "a landed write with a failed audit tail must NOT surface as isError (spec §8 \
             both-halves): {msg}"
        ),
    }
    let text = std::fs::read_to_string(dir.join("signoff.md")).expect("the main write landed");
    assert!(
        text.lines().any(|l| l.starts_with("Worker signoff (probe)")),
        "the signoff line is on disk while the result says success"
    );
    assert!(
        !dir.join(".audit").is_dir(),
        "the planted file was not silently converted — the failure is real"
    );
}

/// The other half of F1: with a healthy audit the success shape is
/// byte-identical to the pre-F1 result — the `audit_error` key does not
/// exist at all (existing frame pins depend on this).
#[test]
fn a_healthy_append_success_carries_no_audit_error_key() {
    let dir = seeded_root("f1-healthy");
    let mut s = handshaken_server(&dir, "probe");
    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Ok(v) => {
            assert_eq!(v["appended"], json!(true));
            assert!(v.get("audit_error").is_none(), "healthy success carries no audit_error: {v}");
            let mut keys: Vec<String> = v
                .as_object()
                .expect("result is an object")
                .keys()
                .map(|k| k.to_string())
                .collect();
            keys.sort();
            assert_eq!(
                keys,
                ["appended".to_string(), "entry".to_string(), "path".to_string()],
                "the success shape without an audit failure is unchanged"
            );
        }
        HandlerResult::Err(msg) => panic!("a healthy append must succeed: {msg}"),
    }
    // And the audit trail really landed (whatever the root's local
    // date names it), so the absence above is not an artifact of the
    // audit never running.
    let trail = read_single_audit_line(&dir);
    assert!(
        trail.contains("\"tool\":\"append_signoff\""),
        "the healthy audit entry landed: {trail}"
    );
}

/// F2 (signoffs/review_Wave4OKF.md 2b) — the takeover half of the
/// mutual-exclusion guarantee: a lock file planted with a FRESH record
/// is never removed by the stale branch. The call must exhaust the
/// bounded retry, end in the pinned already-exists refusal, and leave
/// the holder's lock file on disk, byte-identical. (The deterministic
/// replay of the review's A/B interleaving lives with the fix in
/// `src/tools/signoff_append.rs`'s unit test.)
#[test]
fn fresh_record_never_taken_over_bounded_retry_refuses() {
    let dir = seeded_root("f2-stale-branch");
    let mut s = handshaken_server(&dir, "probe");
    let locks_dir = dir.join(".locks");
    std::fs::create_dir_all(&locks_dir).unwrap();
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let planted = format!(
        "pid={} nonce=90210 epoch={epoch} session=s-live-holder\n",
        std::process::id() + 7
    );
    std::fs::write(locks_dir.join("signoff.md.lock"), &planted).unwrap();

    match signoff_append::append_signoff(&mut s, "probe", &full_args()) {
        HandlerResult::Err(msg) => assert!(
            msg.contains("held by another writer"),
            "the exhausted retry ends in the pinned already-exists refusal: {msg}"
        ),
        HandlerResult::Ok(_) => panic!("the stale branch removed a FRESH holder's lock and wrote"),
    }
    assert_eq!(
        std::fs::read_to_string(locks_dir.join("signoff.md.lock"))
            .expect("the fresh lock file is still present"),
        planted,
        "the fresh record is byte-identical — the stale branch never touched it"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("signoff.md")).unwrap(),
        SEED,
        "refusal wrote zero bytes"
    );
}

/// The one line in today's audit trail: the `.audit/` directory holds
/// one `<local-date>.jsonl` per day and this root saw exactly one
/// mutating write. The file NAME is the root's local date (writer's
/// rule), so the trail is enumerated, never date-guessed.
fn read_single_audit_line(dir: &std::path::Path) -> String {
    let rd = std::fs::read_dir(dir.join(".audit"))
        .expect("the healthy audit path produced its jsonl file");
    let mut lines: Vec<String> = Vec::new();
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".jsonl") {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            lines.extend(text.lines().map(|l| l.to_string()));
        }
    }
    assert_eq!(lines.len(), 1, "exactly the append's own audit entry: {lines:?}");
    lines.pop().unwrap()
}
