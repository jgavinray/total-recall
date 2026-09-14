//! §8 tests 13-17 for the derived recall index (spec.md:365-369), driven
//! through the module-level entry points, plus the primitives the wave
//! hand-rolled and must not silently get wrong.
//!
//! Adaptations, stated rather than hidden:
//! - **15 is genuinely cross-process**: the writer is the REAL
//!   `totalrecall` binary over stdio (the tool already wired today,
//!   `append_signoff`), and the reader is `recall` in this process on the
//!   same root — the pair the spec's "no divergence served" claim is
//!   actually about. A frame test for the wave-4 tools themselves cannot
//!   exist yet: they reach `tools/call` when the orchestrator merges the
//!   registry, and `tests/rpc_tests.rs` owns the registry pin.
//! - **14 tampers three ways**, and the strong one is a report whose
//!   fingerprints still match the live files while its entry lists have
//!   been emptied. If any answer could come from the cache, that tamper
//!   would silently cost every match; the assertion is that the results
//!   are byte-identical to the untampered search.
//! - **confinement** carries the weight `gate_w4` test 3 can no longer
//!   carry: that fixture's authorized correction uses a signoff-only
//!   term, so the *contrast* — the same term found in two buckets under
//!   one scope and in one under another — is pinned here
//!   (`scope_confinement_is_what_narrows_the_answer`).
//! - the unreadable-file tripwire: everything planted outside the buckets
//!   is chmod 000. A search that succeeds (rather than failing loud) while
//!   those files exist is proof they were never opened, not an absence of
//!   evidence.
//!
//! Every test owns its temp root; nothing here ever touches the real
//! memory root (`~/dev/exomemory/`).

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use totalrecall::config;
use totalrecall::rpc::{HandlerResult, Server};
use totalrecall::tools::index::{self, FORMAT_VERSION, INDEX_DIR, MANIFEST_NAME};
use totalrecall::tools::recall::recall;
use totalrecall::tools::ticks;
use serde_json::{json, Value};

/// The worker-signoff bucket, in the shipped line format.
const SIGNOFF: &str = r#"# Signoff

Last updated: 2026-09-13 (08:20 PDT) · by this session

## If you read nothing else
1. **First item** — prose one

## Worker sessions — sign off here as you go

Worker signoff (alpha) | done: yes | unpushed: none | awaits human: none | still running: no | kaibo review: n/a (no code changes) | workflow: memory-kernel | ts: 2026-09-12T23:41:07Z | session: s-a
"#;

const DAY: &str = "# Day\n\nDecision: bring the Rust port up first (reference parity)\nParser edge BLOCKED: version mismatch in the fixture loader\n";
const BRIEF: &str = "# Brief parser\n\nParser edge BLOCKED: version mismatch in the fixture loader\n";

/// The only bucket paths that exist (spec §3's enum, §2's storage table).
const BUCKETS: [&str; 3] = ["signoff.md", "2026-09-13.md", "briefs/parser.md"];

/// Content planted OUTSIDE every bucket. It carries the search terms the
/// confinement tests use, so a leak would be visible as an excerpt.
const OUTSIDE: &str = "Parser edge BLOCKED and Decision: quilt-marker content from outside the buckets\n";

fn root_for(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("totalrecall-index-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Seed the three buckets, and plant the same-looking content in every
/// directory the server is forbidden to search — then make that content
/// unreadable, so "the search succeeded" is itself the evidence that none
/// of it was opened.
fn seed(dir: &Path) {
    std::fs::write(dir.join("signoff.md"), SIGNOFF).unwrap();
    std::fs::write(dir.join("2026-09-13.md"), DAY).unwrap();
    std::fs::create_dir_all(dir.join("briefs")).unwrap();
    std::fs::write(dir.join("briefs/parser.md"), BRIEF).unwrap();
    for outside in [
        "wiki/parser-notes.md",
        "inbox/intake.md",
        "topics/memory.md",
        "signoffs/review_okf.md",
        ".audit/2026-09-13.jsonl",
        ".claims/archived.md",
        ".state/leak.md",
    ] {
        let path = dir.join(outside);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, OUTSIDE).unwrap();
        seal(&path);
    }
    config::init_state(dir).unwrap();
}

/// Remove every access bit: reading this is an EACCES, and `recall`
/// refuses loudly on an unreadable bucket file — so an `Ok` result while
/// a sealed file holds the query's text means the file was never opened.
fn seal(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
}

fn server_for(dir: &Path) -> Server {
    Server::new_server(dir)
}

fn ok(result: HandlerResult) -> Value {
    match result {
        HandlerResult::Ok(value) => value,
        HandlerResult::Err(err) => panic!("expected success, got refusal: {err}"),
    }
}

fn refusal(result: HandlerResult) -> String {
    match result {
        HandlerResult::Err(err) => err,
        HandlerResult::Ok(value) => panic!("expected refusal, got: {value}"),
    }
}

fn search(server: &mut Server, arguments: Value) -> Value {
    ok(recall(server, "probe", &arguments))
}

fn report(root: &Path) -> Option<Value> {
    index::read_manifest_json(root)
}

fn root_report_exists(root: &Path) -> bool {
    root.join(INDEX_DIR).join(MANIFEST_NAME).is_file()
}

fn entry_for<'a>(manifest: &'a Value, rel: &str) -> &'a Value {
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"].as_str() == Some(rel))
        .unwrap_or_else(|| panic!("no entry for {rel} in the derived report"))
}

fn live_fingerprint(root: &Path, rel: &str) -> (i64, u64, u64) {
    let meta = std::fs::metadata(root.join(rel)).unwrap();
    (meta.mtime(), meta.len(), meta.ino())
}

fn live_lines(root: &Path, rel: &str) -> Vec<String> {
    index::split_lines(&std::fs::read_to_string(root.join(rel)).unwrap())
}

fn write_manifest(root: &Path, manifest: &Value) {
    std::fs::write(
        root.join(INDEX_DIR).join(MANIFEST_NAME),
        serde_json::to_string_pretty(manifest).unwrap(),
    )
    .unwrap();
}

/// Every (path, excerpt) pair a search returned, for byte-level
/// comparison across cache states.
fn hits(value: &Value) -> Vec<(String, String)> {
    value["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap().to_string(),
                r["excerpt"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// §8 test 13 — index rebuild on direct edit
// ---------------------------------------------------------------------------

#[test]
fn t13_direct_edit_invalidates_the_index_and_the_report_follows_the_disk() {
    let dir = root_for("t13");
    seed(&dir);
    let mut server = server_for(&dir);

    let before = search(&mut server, json!({"query": "Parser edge"}));
    assert_eq!(before["count"], 2, "day file + brief");
    assert!(
        root_report_exists(&dir),
        "a search leaves the derived report behind"
    );
    let (mtime, size, inode) = live_fingerprint(&dir, "2026-09-13.md");
    let manifest = report(&dir).expect("report written");
    let entry = entry_for(&manifest, "2026-09-13.md");
    assert_eq!(entry["mtime"].as_i64(), Some(mtime));
    assert_eq!(entry["size"].as_u64(), Some(size));
    assert_eq!(entry["inode"].as_u64(), Some(inode));
    assert_eq!(entry["format_version"].as_u64(), Some(FORMAT_VERSION));
    assert_eq!(
        entry["line_count"].as_u64().unwrap() as usize,
        entry["lines"].as_array().unwrap().len(),
        "the fingerprint's line_count IS the parsed entry list"
    );

    // The direct edit: a writer that never went through the server.
    let day = dir.join("2026-09-13.md");
    std::fs::write(
        &day,
        format!("{DAY}\nDecision: the index must never outrank the file\n"),
    )
    .unwrap();
    let (mtime2, size2, _) = live_fingerprint(&dir, "2026-09-13.md");
    assert_ne!((mtime, size), (mtime2, size2), "the edit moved the fingerprint");

    let after = search(&mut server, json!({"query": "never outrank the file"}));
    assert_eq!(after["count"], 1, "the new line is FOUND");
    assert_eq!(after["results"][0]["path"], "2026-09-13.md");

    // And the invalidation is observable, not just implied: the report on
    // disk now records the post-edit identity of the file it describes.
    let refreshed = report(&dir).expect("report still there");
    let entry = entry_for(&refreshed, "2026-09-13.md");
    assert_eq!(
        entry["mtime"].as_i64(),
        Some(mtime2),
        "the cache was rebuilt against the edited file"
    );
    assert_eq!(entry["size"].as_u64(), Some(size2));
}

// ---------------------------------------------------------------------------
// §8 test 14 — stale / corrupt index never served as fact
// ---------------------------------------------------------------------------

#[test]
fn t14_corrupt_truncated_and_tampered_reports_are_never_served() {
    let dir = root_for("t14");
    seed(&dir);
    let mut server = server_for(&dir);
    let arguments = json!({"query": "Parser edge"});
    let untampered = search(&mut server, arguments.clone());
    assert_eq!(untampered["count"], 2);
    let good = report(&dir).expect("report");
    for (path, excerpt) in hits(&untampered) {
        assert!(live_lines(&dir, &path).contains(&excerpt));
    }

    // (a) truncated mid-token: not parseable at all.
    std::fs::write(
        dir.join(INDEX_DIR).join(MANIFEST_NAME),
        &serde_json::to_string(&good).unwrap()[..37],
    )
    .unwrap();
    index::invalidate(&dir); // a fresh process does not inherit the memo
    let reread = search(&mut server, arguments.clone());
    assert_eq!(
        hits(&reread),
        hits(&untampered),
        "a truncated cache costs no matches"
    );
    assert!(report(&dir).is_some(), "and it was replaced, not left broken");

    // (b) the strong tamper: entry lists emptied while the fingerprints
    // still claim the live files. A cache that could answer would report
    // an empty search; the disk answers instead.
    let mut emptied = good.clone();
    for entry in emptied["files"].as_array_mut().unwrap() {
        entry["lines"] = json!([]);
        entry["line_count"] = json!(0);
    }
    write_manifest(&dir, &emptied);
    index::invalidate(&dir);
    let after = search(&mut server, arguments.clone());
    assert_eq!(
        hits(&after),
        hits(&untampered),
        "a fingerprint-preserving edit to the entry lists changes no answer"
    );
    for (path, excerpt) in hits(&after) {
        assert!(
            live_lines(&dir, &path).contains(&excerpt),
            "{path}: served text must exist in the live file"
        );
    }
    let repaired = report(&dir).expect("rebuilt report");
    assert_eq!(
        entry_for(&repaired, "2026-09-13.md")["lines"]
            .as_array()
            .unwrap()
            .len(),
        live_lines(&dir, "2026-09-13.md").len(),
        "the emptied entry list was rebuilt from the file, not from the lie"
    );

    // (c) a report from a future schema: stale by definition, never
    // misread (hazard 5) — and still not an error.
    let mut future = good.clone();
    future["format_version"] = json!(FORMAT_VERSION + 7);
    write_manifest(&dir, &future);
    index::invalidate(&dir);
    let after = search(&mut server, arguments);
    assert_eq!(hits(&after), hits(&untampered));
    assert_eq!(
        report(&dir).expect("rebuilt")["format_version"].as_u64(),
        Some(FORMAT_VERSION),
        "the version-mismatched report is replaced by the current schema"
    );
}

// ---------------------------------------------------------------------------
// §8 test 15 — two processes, one root, one index
// ---------------------------------------------------------------------------

#[test]
fn t15_two_processes_one_root_the_reader_never_diverges() {
    let dir = root_for("t15");
    seed(&dir);
    let mut reader = server_for(&dir);

    // Process A warms the cache through the tool.
    let query = json!({"query": "Worker signoff (writer-b)"});
    let before = search(&mut reader, query.clone());
    assert_eq!(before["count"], 0, "the line does not exist yet");
    assert!(root_report_exists(&dir), "A built the derived report");

    // Process B: the REAL binary, its own handshake, its own append, over
    // stdio on the same root.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_totalrecall"))
        .arg("--root")
        .arg(dir.display().to_string())
        .env_remove("EXOMEMORY_DIR")
        .env_remove("EXO_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the server binary builds for tests");
    {
        let stdin = child.stdin.as_mut().unwrap();
        for frame in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"writer-b","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_signoff","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"writer-b","workflow":"index-cross","done":"yes"}}}),
        ] {
            writeln!(stdin, "{frame}").unwrap();
        }
        stdin.flush().unwrap();
    } // stdin drops -> EOF, the child exits its loop
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "the writer process exits clean (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !stdout.contains("\"isError\":true"),
        "B's handshake and append both succeeded: {stdout}"
    );
    assert!(
        std::fs::read_to_string(dir.join("signoff.md"))
            .unwrap()
            .contains("Worker signoff (writer-b)"),
        "B's line is on disk"
    );

    // A must find it: its cached snapshot's fingerprint for signoff.md is
    // now the old file's, so the cache cannot serve the stale answer.
    let after = search(&mut reader, query);
    assert_eq!(after["count"], 1, "no divergence served across processes");
    assert_eq!(after["results"][0]["path"], "signoff.md");
    assert!(after["results"][0]["excerpt"]
        .as_str()
        .unwrap()
        .contains("workflow: index-cross"));

    // And the shared report now describes the file B left behind.
    let (mtime, size, inode) = live_fingerprint(&dir, "signoff.md");
    let manifest = report(&dir).expect("report");
    let entry = entry_for(&manifest, "signoff.md");
    assert_eq!(entry["mtime"].as_i64(), Some(mtime));
    assert_eq!(entry["size"].as_u64(), Some(size));
    assert_eq!(entry["inode"].as_u64(), Some(inode));
}

// ---------------------------------------------------------------------------
// §8 test 16 — index never authoritative (delete it entirely)
// ---------------------------------------------------------------------------

#[test]
fn t16_deleting_the_index_changes_nothing() {
    let dir = root_for("t16");
    seed(&dir);
    let mut server = server_for(&dir);
    let arguments = json!({"query": "Decision"});
    let with_cache = search(&mut server, arguments.clone());
    assert_eq!(with_cache["count"], 1);
    assert!(root_report_exists(&dir));

    // Housekeeping is allowed to delete the cache at any time (§5): the
    // answer must not change, and the cache comes back from the disk.
    std::fs::remove_dir_all(dir.join(INDEX_DIR)).unwrap();
    index::invalidate(&dir); // the cold-start analogue: nothing memoized either
    assert!(!root_report_exists(&dir));
    let without_cache = search(&mut server, arguments.clone());
    assert_eq!(
        hits(&without_cache),
        hits(&with_cache),
        "results are the disk's, with or without the cache"
    );
    assert!(
        root_report_exists(&dir),
        "the report is derived again, not mourned"
    );

    // The same proof on a forced cold build, memo aside.
    let cold = index::snapshot_with(&dir, false).expect("cold rebuild never fails on a readable root");
    assert_eq!(cold.files.len(), 3, "one snapshot entry per bucket file");
    for rel in BUCKETS {
        assert!(cold.files.contains_key(rel), "{rel} missing from the cold build");
    }
}

// ---------------------------------------------------------------------------
// §8 test 17 — path confinement with the index
// ---------------------------------------------------------------------------

#[test]
fn t17_crafted_escapes_are_refused_and_nothing_outside_is_read() {
    let dir = root_for("t17");
    seed(&dir);
    let mut server = server_for(&dir);

    // The out-of-bucket content is sealed AND carries the same words the
    // buckets do; every refusal below and every `Ok` with no leak is
    // evidence about what was opened, not an assumption.
    for crafted in [
        "wiki",
        "topics",
        "signoffs",
        "inbox",
        "Wiki",
        "DAYFILE",
        "dayfile ",
        "",
        "../../etc",
        "/etc",
        "briefs/..",
        "../signoff.md",
        "signoff.md",
        ".index",
        ".audit",
    ] {
        let msg = refusal(recall(
            &mut server,
            "probe",
            &json!({"query": "Decision", "scope": crafted}),
        ));
        assert!(
            msg.starts_with("recall refused: scope must be one of dayfile|signoff|briefs|all"),
            "crafted scope {crafted:?} must be refused in the pinned style, got: {msg}"
        );
        assert!(!msg.contains("quilt-marker"), "refusals quote no bucket content");
    }

    // The schema has no path parameter, so an escape attempt arrives as a
    // field — and is refused as one.
    let msg = refusal(recall(
        &mut server,
        "probe",
        &json!({"query": "Decision", "path": "../../etc/passwd"}),
    ));
    assert!(
        msg.contains("unknown field") && msg.contains("\"path\"") && msg.contains("query, scope, workflow, since"),
        "the refusal must name the field and the allowed set, got: {msg}"
    );
    let msg = refusal(recall(
        &mut server,
        "probe",
        &json!({"query": "Decision", "scope": "all", "workflow": ""}),
    ));
    assert!(msg.contains("'workflow'"), "got: {msg}");
    for bad_since in [
        "2026-9-13",
        "last week",
        "2026-13-40",
        "1970-01-01T00:00:00Z",
    ] {
        let msg = refusal(recall(
            &mut server,
            "probe",
            &json!({"query": "Decision", "since": bad_since}),
        ));
        assert!(msg.contains("since must be an ISO date"), "got: {msg}");
    }

    // A path-shaped QUERY is search text, never a path: searched as text,
    // it matches nothing and opens nothing.
    for query in [
        "../../etc/passwd",
        "/etc/passwd",
        "..",
        "wiki/parser-notes.md",
        "C:\\..\\windows",
    ] {
        let v = search(&mut server, json!({"query": query}));
        assert_eq!(v["count"], 0, "path-shaped query {query:?} must match nothing");
    }

    // A regex that matches every line still reports only bucket paths.
    let v = search(&mut server, json!({"query": "re:.*"}));
    let paths: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert!(!paths.is_empty(), "the buckets do have content to find");
    for path in paths {
        assert!(
            BUCKETS.contains(&path),
            "{path} is not a bucket yet was searched"
        );
        assert!(!path.contains("..") && !path.starts_with('/'));
    }

    // A symlinked bucket FILE is not a bucket entry at all (enumeration
    // takes regular files), so the sealed target stays shut.
    let dir = root_for("t17-symlink-file");
    seed(&dir);
    let outside = root_for("t17-outside");
    let secret = outside.join("secret.md");
    std::fs::write(&secret, OUTSIDE).unwrap();
    seal(&secret);
    std::os::unix::fs::symlink(&secret, dir.join("briefs/leak.md")).unwrap();
    let mut server = server_for(&dir);
    let v = search(&mut server, json!({"query": "quilt-marker"}));
    assert_eq!(v["count"], 0, "the symlinked-out file is not a bucket entry");
    let v = search(&mut server, json!({"query": "Parser edge", "scope": "briefs"}));
    assert_eq!(v["count"], 1, "only the real brief");
    assert_eq!(v["results"][0]["path"], "briefs/parser.md");
}

#[test]
fn t17b_a_bucket_that_resolves_outside_the_root_is_a_loud_refusal() {
    let dir = root_for("t17b");
    std::fs::write(dir.join("signoff.md"), SIGNOFF).unwrap();
    let outside = root_for("t17b-outside");
    std::fs::write(outside.join("anything.md"), OUTSIDE).unwrap();
    // briefs/ is itself a pointer out of the root.
    std::os::unix::fs::symlink(&outside, dir.join("briefs")).unwrap();
    let mut server = server_for(&dir);
    for scope in ["all", "briefs"] {
        let msg = refusal(recall(
            &mut server,
            "probe",
            &json!({"query": "Decision", "scope": scope}),
        ));
        assert!(
            msg.contains("path confinement refused")
                && msg.contains("outside the configured memory root"),
            "an escaping bucket must be refused LOUDLY and name the path, got: {msg}"
        );
        assert!(msg.contains("briefs"), "the refusal names what escaped: {msg}");
        assert!(!msg.contains("quilt-marker"), "no content leaks into a refusal");
    }
}

// ---------------------------------------------------------------------------
// The confinement CONTRAST (carries what the corrected gate_w4 test 3 cannot)
// ---------------------------------------------------------------------------

#[test]
fn scope_confinement_is_what_narrows_the_answer() {
    let dir = root_for("scope-contrast");
    seed(&dir);
    let mut server = server_for(&dir);
    let term = json!({"query": "Parser edge BLOCKED"});

    let all = hits(&search(&mut server, term.clone()));
    assert_eq!(
        all,
        vec![
            (
                "2026-09-13.md".to_string(),
                "Parser edge BLOCKED: version mismatch in the fixture loader".to_string()
            ),
            (
                "briefs/parser.md".to_string(),
                "Parser edge BLOCKED: version mismatch in the fixture loader".to_string()
            ),
        ],
        "scope=all finds both buckets — and NOT the identically-worded wiki/inbox/topics/signoffs copies"
    );

    let one = hits(&search(
        &mut server,
        json!({"query": "Parser edge BLOCKED", "scope": "briefs"}),
    ));
    assert_eq!(one.len(), 1, "the scope is what narrows it");
    assert_eq!(one[0].0, "briefs/parser.md");

    let none = hits(&search(
        &mut server,
        json!({"query": "Parser edge BLOCKED", "scope": "signoff"}),
    ));
    assert!(none.is_empty(), "the named bucket simply has no such line");

    // `since` is a lower bound on the ts DATE, and the ts is the live
    // mtime — so a date past today bounds everything out.
    let v = search(&mut server, json!({"query": "Parser edge", "since": "1999-01-01"}));
    assert_eq!(v["count"], 2, "a bound in the past admits all of them");
    let v = search(&mut server, json!({"query": "Parser edge", "since": "2999-01-01"}));
    assert_eq!(v["count"], 0, "a bound past the live mtime excludes them");
}

// ---------------------------------------------------------------------------
// The report describes the cache it claims to describe
// ---------------------------------------------------------------------------

#[test]
fn the_derived_report_describes_exactly_the_three_buckets() {
    let dir = root_for("report-shape");
    seed(&dir);
    let mut server = server_for(&dir);
    search(&mut server, json!({"query": "Decision"}));
    let manifest = report(&dir).expect("a search writes the derived report");
    assert_eq!(manifest["format_version"].as_u64(), Some(FORMAT_VERSION));
    assert!(manifest["built_at"].as_str().unwrap().ends_with('Z'));
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(files.len(), 3, "one entry per bucket file, nothing else");
    let mut seen: Vec<&str> = files
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    seen.sort_unstable();
    let mut expected = BUCKETS.to_vec();
    expected.sort_unstable();
    assert_eq!(seen, expected);
    for entry in files {
        let rel = entry["path"].as_str().unwrap();
        let (mtime, size, inode) = live_fingerprint(&dir, rel);
        assert_eq!(entry["mtime"].as_i64(), Some(mtime), "{rel} mtime");
        assert_eq!(entry["size"].as_u64(), Some(size), "{rel} size");
        assert_eq!(entry["inode"].as_u64(), Some(inode), "{rel} inode");
        let lines = entry["lines"].as_array().unwrap();
        assert_eq!(entry["line_count"].as_u64().unwrap() as usize, lines.len());
        assert_eq!(lines.len(), live_lines(&dir, rel).len(), "{rel} entry list");
    }
    // The lock it was written under is released, not left claimed.
    assert!(
        !dir.join(".locks").join("index.lock").exists(),
        "the report write releases its lock"
    );
}

// ---------------------------------------------------------------------------
// The hand-rolled primitives (std + serde_json only — §2, §13 R3)
// ---------------------------------------------------------------------------

#[test]
fn the_std_calendar_inverse_agrees_with_the_system_clock() {
    // Values taken from the OS (`date -u -j -f %Y-%m-%dT%H:%M:%SZ … +%s`),
    // not from the code under test. `last_tick`'s 45-minute boundary is
    // only as honest as this function.
    assert_eq!(ticks::parse_utc_iso8601("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(
        ticks::parse_utc_iso8601("2000-03-01T00:00:00Z"),
        Some(951_868_800)
    );
    assert_eq!(
        ticks::parse_utc_iso8601("2026-09-13T06:10:00Z"),
        Some(1_789_279_800),
        "the spec §3 example ts, as the system reads it"
    );
    // Shape discipline: a tick in any other shape is not a tick.
    for junk in [
        "2026-09-13T06:10:00",
        "2026-09-13 06:10:00Z",
        "2026-09-13T06:10:00z",
        "2026-13-13T06:10:00Z",
        "2026-09-32T06:10:00Z",
        "2026-09-13T24:00:00Z",
        "2026-09-13T06:60:00Z",
        "2026-9-13T06:10:00Z",
        "",
    ] {
        assert_eq!(
            ticks::parse_utc_iso8601(junk),
            None,
            "{junk:?} is not an ISO-8601Z instant"
        );
    }
}

#[test]
fn the_std_regex_engine_covers_the_pinned_search_modes() {
    let find = |query: &str, line: &str| {
        index::Query::compile(query)
            .and_then(|q| q.matches(line))
            .unwrap_or_else(|err| panic!("query {query:?} must compile: {err}"))
    };
    // Substring mode is case-insensitive and literal — a `(` in the query
    // is a character, not a group.
    assert!(find("parser EDGE", "Parser edge BLOCKED"));
    assert!(find("BLOCKED:", "Parser edge BLOCKED: version"));
    assert!(!find("blocked silently", "Parser edge BLOCKED: version"));
    assert!(find("(reference parity)", "Decision (reference parity)"));
    assert!(!find("(parity", "plain text with no parenthesis"));

    // Regex mode: the pinned `re:` prefix, case-insensitive.
    assert!(find("re:Parser edge (BLOCK|BLOCKED)", "parser edge blocked: yes"));
    // The shipped signoff line has a space after the colon, so a real
    // worker-search query says so — and adjacency means what it says.
    assert!(find("re:done:\\s?(yes|no)", "| done: no | unpushed"));
    assert!(
        !find("re:done:(yes|no)", "| done: no | unpushed"),
        "a space sits between the colon and the value"
    );
    assert!(find("re:done:(yes|no)", "| done:yes |"), "adjacent is adjacent");
    assert!(
        !find("re:done:\\s?(yes|no)", "| done: maybe | unpushed"),
        "neither alternative is `maybe`"
    );
    assert!(find("re:^# Brief parser$", "# Brief parser"));
    assert!(!find("re:^Brief parser$", "# Brief parser"));
    assert!(find("re:[0-9]{4}-[0-9]{2}-[0-9]{2}", "ts: 2026-09-12T23:41:07Z"));
    assert!(!find("re:[0-9]{5}", "only four digits: 1234"));
    assert!(find("re:w[ao]rk", "work and woik both count"));
    assert!(find("re:\\d\\d-", "stamped 26-09-12"));
    assert!(find("re:a+b*c?", "aaabc"), "counts are honoured");
    assert!(find("re:a*b", "b"), "a starred body may match zero times");
    assert!(
        !find("re:a+b*c?", "b c d"),
        "`+` demands an `a`, and there is none"
    );
    // Negated classes, and the case-fold is honest: `Z` in a pattern
    // matches a lowercase `z` in the line.
    assert!(find("re:[^0-9]5", "release 5"), "a space is not a digit");
    assert!(
        !find("re:[^0-9]5", "12345"),
        "every character before a digit is a digit here"
    );
    assert!(find("re:[^a-z]Z", "release Z"), "case-folded negated class");
    assert!(
        !find("re:[^a-z]Q", "lowercase text only"),
        "no `q` to follow the non-letter"
    );

    // A pattern the engine cannot honour is refused LOUDLY — never an
    // empty result set pretending to be an answer.
    for broken in [
        "re:(unclosed",
        "re:unbalanced)",
        "re:[z-a]",
        "re:[unclosed class",
        "re:a**",
        "re:*leading",
        "re:a{",
        "re:a{2",
        "re:\\q",
        "re:^*anchor",
    ] {
        let err = index::Query::compile(broken)
            .err()
            .unwrap_or_else(|| panic!("{broken:?} must be refused, not compiled into a silent search"));
        assert!(
            !err.is_empty(),
            "the refusal for {broken:?} must explain itself: {err}"
        );
    }
    assert!(
        index::Query::compile("re:").is_err(),
        "an empty pattern is not a search"
    );
}

/// F6 (finding): `REP_CAP` is the budget an UNBOUNDED repetition may
/// enumerate. A COUNTED bound above it used to be clamped silently, so
/// the engine answered "no match" for a pattern it had been asked to
/// honour and could not — the both-halves lie the spec refuses
/// everywhere else. It is a compile refusal now, in the same voice as
/// every other unsupported pattern.
#[test]
fn a_counted_repetition_over_the_rep_cap_is_refused_not_clamped() {
    for broken in ["re:a{300}", "re:a{300,}", "re:a{2,300}", "re:(ab){1000,2000}"] {
        let err = index::Query::compile(broken)
            .err()
            .unwrap_or_else(|| panic!("{broken:?} must be refused, not clamped into a silent answer"));
        assert!(
            err.starts_with("regex is malformed at position"),
            "the refusal speaks the engine's own voice: {err}"
        );
        assert!(
            err.contains("counted repetition") && err.contains("256"),
            "the refusal names the offending repetition and the cap it broke: {err}"
        );
    }
    // The clamp was never a licence to answer a smaller question: a
    // 300-repetition over a haystack that could satisfy it is refused,
    // not answered as the empty set.
    let err = index::Query::compile("re:a{300}")
        .and_then(|q| q.matches(&"a".repeat(400)))
        .expect_err("an over-cap counted repetition never reaches an answer");
    assert!(err.contains("counted repetition"), "{err}");

    // At the cap the engine still answers, and answers honestly:
    // 256 reps match 256 and 300 copies of the character, and refuse
    // 255. Boundaries are what a cap is for.
    let matched = |pattern: &str, line: &str| {
        index::Query::compile(pattern)
            .and_then(|q| q.matches(line))
            .unwrap_or_else(|err| panic!("{pattern:?} at the cap must compile and answer: {err}"))
    };
    assert!(matched("re:a{256}", &"a".repeat(256)), "256 reps over 256 chars");
    assert!(matched("re:a{256}", &"a".repeat(300)), "256 reps inside 300 chars");
    assert!(
        !matched("re:a{256}", &"a".repeat(255)),
        "256 reps need 256 characters — the cap honours the pattern, it never stretches it"
    );
    assert!(matched("re:a{254,256}", &"a".repeat(256)), "an over-cap-free range still works");
    assert!(matched("re:a+", &"a".repeat(400)), "the UNBOUNDED case keeps its REP_CAP budget");
}

#[test]
fn a_regex_the_engine_cannot_finish_is_refused_not_answered() {
    // `(a*)*b` is the classic catastrophic case. The engine is budgeted,
    // and exhausting the budget is a refusal naming the pattern — the
    // alternative is a search that quietly reports nothing.
    let haystack = "a".repeat(400);
    let err = index::Query::compile("re:(a*)*b")
        .and_then(|q| q.matches(&haystack))
        .expect_err("the search budget must be exhausted, not answered");
    assert!(
        err.contains("budget"),
        "the refusal must say why it could not search: {err}"
    );
    // Recall surfaces it as an ordinary refusal, so a client sees the
    // reason instead of an empty result.
    let dir = root_for("regex-budget");
    seed(&dir);
    std::fs::write(
        dir.join("2026-09-13.md"),
        format!("{DAY}\n{}\n", "a".repeat(400)),
    )
    .unwrap();
    let mut server = server_for(&dir);
    let msg = refusal(recall(&mut server, "probe", &json!({"query": "re:(a*)*b"})));
    assert!(msg.contains("budget"), "recall must forward the refusal: {msg}");
}
