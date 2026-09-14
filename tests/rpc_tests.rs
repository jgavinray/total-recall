//! §8 companion — wave 1, `rpc` module self-checks (ExoM1).
//!
//! Module-scoped only, per the build contract, and deliberately
//! NON-OVERLAPPING with the orchestrator's gate (`tests/gate_w1.rs`):
//! where the gate pins current-revision initialize, the *named* unknown
//! tool (-32602), `tools/nope` (-32601), `"{not json"` (-32700), and the
//! isError-result/no-side-effect shape of a gated refusal, this file pins
//! the angles the gate leaves open:
//!
//! - initialize echoes the EARLIER implemented revision (2025-11-25) and
//!   falls back to the current one for unknown/missing revisions
//! - structurally invalid frames (valid JSON, no usable method) -> -32600
//!   invalid request — distinct from the gate's -32700 parse error
//! - `tools/call` with no `params.name` at all -> -32602
//! - notification silence: no `id` member -> nothing emitted, including
//!   for a method the server does not implement (JSON-RPC 2.0: a
//!   notification never receives a response)
//! - the refusal carries the EXACT spec §4 text and echoes the id
//! - the §4 gate: `initialize` does NOT grant the handshake; the
//!   `attempted_write_before_handshake` counter counts gate refusals and
//!   nothing else
//! - the wave-1 `tools/list` shape: the registry carries `append_signoff`
//!   with the pinned spec §3 schema
//! - `serve_stdio` end-to-end: frames out on stdout, notifications silent,
//!   clean EOF -> exit 0 (spawns the real binary; this is the one test
//!   coupled to the sibling's `src/main.rs` landing)
//!
//! This file compiles against the `exomem_mcp` lib; until the sibling
//! module (`src/config.rs`, declared by the orchestrator-owned `lib.rs`)
//! lands, the crate does not compile at all — the intended loud failure.

use exomem_mcp::rpc::{self, Server};
use serde_json::{json, Value};
use std::io::Write;

fn rpc_root(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("exomem-rpc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fresh_server(name: &str) -> Server {
    Server::new_server(&rpc_root(name))
}

#[test]
fn initialize_echoes_earlier_implemented_revision() {
    let mut s = fresh_server("init-prev");
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":"a-1","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["result"]["protocolVersion"], "2025-11-25",
        "the server implements 2025-11-25 and must echo the requested earlier revision"
    );
    assert_eq!(v["id"], "a-1", "the id must round-trip as-is");
    assert!(v["error"].is_null());
}

#[test]
fn initialize_unknown_or_missing_revision_falls_back_to_current() {
    let mut s = fresh_server("init-unknown");
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01","capabilities":{}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["result"]["protocolVersion"], "2026-07-28",
        "a revision the server does not implement must be answered with the current one"
    );
    let out = rpc::handle_frame(&mut s, r#"{"jsonrpc":"2.0","id":2,"method":"initialize","params":{}}"#)
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["result"]["protocolVersion"], "2026-07-28",
        "a missing protocolVersion must be answered with the current one"
    );
}

#[test]
fn tools_call_with_no_name_is_32602() {
    let mut s = fresh_server("call-noname");
    // No params.name at all: there is no tool name to look up, so the call
    // fails as invalid params — the same -32602 the gate pins for a NAMED
    // unknown tool (this file covers the nameless angle).
    let err = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{}}"#,
    )
    .unwrap_err();
    let v: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(v["error"]["code"], -32602);
    assert!(v["error"]["message"].as_str().unwrap().contains("unknown tool name"));
    assert_eq!(v["id"], 7);
}

#[test]
fn structurally_invalid_frame_is_32600_not_32700() {
    let mut s = fresh_server("invalid-req");
    // Valid JSON that is not a JSON-RPC request object: parse succeeds, so
    // this is an invalid REQUEST (-32600), not a parse error (-32700, which
    // the gate pins for non-JSON bytes).
    let err = rpc::handle_frame(&mut s, "42").unwrap_err();
    let v: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(v["error"]["code"], -32600);
    // A request object missing `method` is structurally invalid too, and the
    // recoverable id must be preserved in the error frame.
    let err = rpc::handle_frame(&mut s, r#"{"jsonrpc":"2.0","id":5}"#).unwrap_err();
    let v: Value = serde_json::from_str(&err).unwrap();
    assert_eq!(v["error"]["code"], -32600);
    assert_eq!(v["id"], 5, "a recoverable id must be preserved in the error frame");
}

#[test]
fn notifications_emit_nothing_even_when_unimplemented() {
    let mut s = fresh_server("notify");
    // No `id` member -> a notification -> the server MUST NOT emit any
    // response at all (JSON-RPC 2.0), even for a method it does not
    // implement — a notification can never receive an error frame.
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
    )
    .unwrap();
    assert!(out.is_empty(), "a notification must produce no output");
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","method":"resources/subscribe","params":{}}"#,
    )
    .unwrap();
    assert!(out.is_empty(), "an unimplemented-method notification must still be silent");
}

#[test]
fn refusal_carries_exact_spec_text_and_echoes_id() {
    let mut s = fresh_server("refusal-text");
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["id"], 42, "the id must round-trip");
    assert_eq!(
        v["result"]["content"][0]["type"], "text",
        "the pinned refusal content is a text item"
    );
    assert_eq!(
        v["result"]["content"][0]["text"],
        "handshake incomplete — call read_signoff first",
        "the refusal must carry the exact spec §4 text"
    );
}

#[test]
fn initialize_does_not_grant_the_handshake() {
    // Spec §4: the gate is uniform and opens on read_signoff ONLY.
    // initialize is protocol negotiation, not the session handshake — a
    // gated call after initialize must still be refused.
    let mut s = fresh_server("gate-init");
    rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{}}}"#,
    )
    .unwrap();
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["result"]["content"][0]["text"],
        "handshake incomplete — call read_signoff first",
        "initialize must not open the gate"
    );
}

#[test]
fn gate_opens_when_handshaken_and_counter_stays_put() {
    // The handshaken map is the process-local gate state (spec §4
    // pseudocode: `handshaken[session] = True` on read_signoff). The
    // later-wave read_signoff writes it; here the test sets the pinned
    // field directly. Once handshaken, the gate must NOT fire — the
    // wave-1 deferral is a normal isError result (the append body is a
    // later wave, and the protocol layer never claims a write it did not
    // make) and it must NOT count as a pre-handshake attempt.
    let mut s = fresh_server("gate-open");
    s.handshaken.insert(s.session_id.clone(), true);
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["result"]["isError"].as_bool().unwrap(), "still a normal result, never an error object");
    assert!(
        v["result"]["content"][0]["text"]
            != "handshake incomplete — call read_signoff first",
        "the gate refusal must be gone once handshaken"
    );
    assert!(
        !s.attempted_write_before_handshake.contains_key(&s.session_id),
        "a post-handshake call is not a pre-handshake attempt"
    );
}

#[test]
fn gate_refusals_are_counted_per_session() {
    // Spec §4: attempted_write_before_handshake is "observable at the
    // gate" — the counter is exactly what the session_compliance report
    // will read in a later wave.
    let mut s = fresh_server("gate-count");
    let frame = r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#;
    let _ = rpc::handle_frame(&mut s, frame).unwrap();
    assert_eq!(s.attempted_write_before_handshake.get(&s.session_id), Some(&1));
    let _ = rpc::handle_frame(&mut s, frame).unwrap();
    assert_eq!(
        s.attempted_write_before_handshake.get(&s.session_id),
        Some(&2),
        "every refused pre-handshake attempt must count"
    );
}

#[test]
fn tools_list_wave1_registry_is_the_pinned_shape() {
    // Wave 1 registers the gated member of the handshake pair with the
    // spec §3 shape; the free-function builder (contract module API) is
    // exercised here while the rest of this file uses Server::new_server.
    let mut s = rpc::new_server(&rpc_root("tools-list"));
    let out = rpc::handle_frame(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    let tools = v["result"]["tools"].as_array().expect("tools must be an array");
    assert_eq!(tools.len(), 1, "the wave-1 registry carries exactly append_signoff");
    assert_eq!(tools[0]["name"], "append_signoff");
    assert!(!tools[0]["description"].as_str().unwrap().is_empty());
    let schema = &tools[0]["inputSchema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(
        schema["required"],
        json!(["role", "workflow", "done"]),
        "the required-fields order is part of the pinned shape"
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["done"]["enum"], json!(["yes", "no"]));
    assert_eq!(schema["properties"]["role"]["type"], "string");
}

#[test]
fn serve_stdio_emits_frames_keeps_notifications_silent_and_exits_zero() {
    // End-to-end over the real binary (the sibling's src/main.rs; this is
    // the one test coupled to that file landing). The child's env is
    // isolated: ambient EXOMEMORY_DIR / EXO_DIR must not leak in — the
    // --root flag is the top of the precedence chain (contract).
    let dir = rpc_root("stdio");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_exomem-mcp"));
    cmd.arg("--root")
        .arg(dir.display().to_string())
        .env_remove("EXOMEMORY_DIR")
        .env_remove("EXO_DIR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().unwrap();
    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        stdin
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2026-07-28\",\"capabilities\":{}}}\n",
            )
            .unwrap();
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"resources/list\"}\n")
            .unwrap();
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .unwrap();
        stdin.write_all(b"{not json\n").unwrap();
    } // stdin drops here -> EOF
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "clean EOF must exit 0 (stderr: {})",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "exactly three frames on stdout — the notification emits nothing — got: {stdout}"
    );
    let init: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2026-07-28");
    assert!(lines[1].contains("-32601"), "unknown method frame: {}", lines[1]);
    assert!(lines[2].contains("-32700"), "parse-error frame: {}", lines[2]);
}
