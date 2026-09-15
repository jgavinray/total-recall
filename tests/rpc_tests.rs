//! §8 companion — wave 1 + wave-2 rewire, `rpc` module self-checks (ExoM1).
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
//! - the merged `tools/list` registry: exactly the six implemented tools
//!   by name, the gate-opener advertised first, with the pinned spec §3
//!   schemas
//! - the frame-level MCP result envelope: success arrives as ONE text
//!   item holding the serialized handler JSON (no `isError`, no raw-field
//!   leak), refusals keep the pinned `isError` + content shape — and a
//!   refused call leaves the root byte-untouched (spec §8 both-halves rule)
//! - `initialize` carries the MCP-mandatory `serverInfo` and the
//!   tools-only capability surface
//! - `serve_stdio` end-to-end: frames out on stdout, notifications silent,
//!   clean EOF -> exit 0 (spawns the real binary; this is the one test
//!   coupled to the sibling's `src/main.rs` landing)
//!
//! This file compiles against the `total-recall` lib (wave 2: all
//! modules landed).

use total_recall::rpc::{self, Server};
use serde_json::{json, Value};
use std::io::Write;

fn rpc_root(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("total-recall-rpc-{name}-{}", std::process::id()));
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
        "handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).",
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
        "handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).",
        "initialize must not open the gate"
    );
}

#[test]
fn gate_opens_when_handshaken_and_counter_stays_put() {
    // The handshaken map is the process-local gate state (spec §4
    // pseudocode: `handshaken[session] = True` on read_signoff). The
    // live path is read_signoff's dispatch entry; here the test sets
    // the pinned map field directly so this protocol-layer check stays
    // self-contained. Once handshaken, the gate must NOT fire — the
    // wave-2 handler performs the REAL append (a normal result, no
    // isError, the line on disk) and a post-handshake call must NOT
    // count as a pre-handshake attempt.
    let mut s = fresh_server("gate-open");
    s.handshaken.insert(s.session_id.clone(), true);
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["id"], 3);
    assert!(
        v["result"]["isError"].is_null(),
        "post-handshake: a success result, never an error object and no refusal"
    );
    // The MCP success envelope (built at dispatch, spec §2): exactly ONE
    // text item holding the serialized handler JSON. Decode it, THEN pin
    // the wave-2 success shape carried inside.
    let content = v["result"]["content"]
        .as_array()
        .expect("success carries a content array");
    assert_eq!(content.len(), 1, "exactly ONE text item");
    assert_eq!(content[0]["type"], "text");
    let payload: Value = serde_json::from_str(content[0]["text"].as_str().unwrap())
        .expect("the text item is the serialized handler result");
    assert_eq!(payload["appended"], true, "the wave-2 success shape");
    assert!(
        payload["entry"].as_str().unwrap().contains("Worker signoff (probe)"),
        "the echoed entry carries the fixed token"
    );
    // The append really happened on disk, stamped with the process-local
    // session (the frame path keys the gate and the stamps on
    // server.session_id). The handler's handshake step also seeds the
    // stock template on a fresh root, so pin the APPENDED line —
    // the last one — not the whole file (file contents are pinned by
    // gate_w2).
    let written = std::fs::read_to_string(s.root.join("signoff.md")).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    let last = lines.last().copied().expect("signoff.md is non-empty");
    assert!(
        last.starts_with("Worker signoff (probe) | done: yes"),
        "the pinned line format: {}",
        last
    );
    assert!(last.contains(&format!("| session: {}", s.session_id)));
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
fn tools_list_registry_carries_the_full_ten_tool_set() {
    // The merged registry: every implemented module's tools, pinned by
    // EXACT name set — the full spec §3 ten-tool surface (the handshake
    // pair, the wave-3 four, and the wave-4 four). The free-function
    // builder (contract module API) is exercised here while the rest of
    // this file uses Server::new_server.
    let mut s = rpc::new_server(&rpc_root("tools-list"));
    let out = rpc::handle_frame(&mut s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    let tools = v["result"]["tools"].as_array().expect("tools must be an array");
    let mut names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("every tool is named"))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "append_signoff",
            "claim_orchestrator",
            "last_tick",
            "log_tick",
            "read_signoff",
            "recall",
            "session_compliance",
            "write_brief",
            "write_dayfile",
            "write_warm_start",
        ],
        "the merged registry carries exactly the spec §3 ten tools"
    );
    assert_eq!(
        tools[0]["name"], "read_signoff",
        "the gate-opener is advertised first (handshake order)"
    );
    assert_eq!(tools[1]["name"], "append_signoff");
    for tool in tools {
        assert!(!tool["description"].as_str().unwrap().is_empty());
    }
    // read_signoff takes no arguments: empty object, closed.
    let read = &tools[0]["inputSchema"];
    assert_eq!(read["type"], "object");
    assert!(
        read["properties"].as_object().map(|p| p.is_empty()).unwrap_or(false),
        "read_signoff's schema carries no properties"
    );
    assert_eq!(read["additionalProperties"], false);
    // append_signoff keeps the pinned spec §3 schema.
    let schema = &tools[1]["inputSchema"];
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
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_total-recall"));
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

#[test]
fn initialize_result_carries_serverinfo_and_tools_only_capabilities() {
    // MCP makes `serverInfo` mandatory on the initialize result; the
    // capability surface stays tools-only — this server advertises no
    // resources (spec §3: absence IS the api).
    let mut s = fresh_server("init-info");
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":"si","method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["result"]["serverInfo"]["name"], "total-recall");
    assert_eq!(v["result"]["serverInfo"]["version"], "0.3.0");
    assert!(
        v["result"]["capabilities"]["tools"].is_object(),
        "the tools capability must be present"
    );
    assert!(
        v["result"]["capabilities"].get("resources").is_none(),
        "no MCP resources surface"
    );
    // The spec §11 prompt layer rides the MCP-native `instructions`
    // channel VERBATIM — omp injects it into the model-facing system
    // prompt (client.ts → getServerInstructions → rebuildSystemPrompt).
    // Pinned line-by-line against §11 so drift between contract and
    // wire is a test failure, not a silent prompt difference.
    let instructions = v["result"]["instructions"]
        .as_str()
        .expect("initialize carries instructions");
    for line in [
        "## Memory protocol (MCP: total-recall)",
        "- First action of every session: mcp__total_recall_read_signoff. No work before it returns.",
        "- Last action before any stop/compact/handoff: mcp__total_recall_append_signoff {role, workflow, done, unpushed, awaits_human, still_running, kaibo_review}.",
        "- Facts come from mcp__total_recall_recall results (dated) or files — never from your context memory.",
        "- The server refuses every append path (including log_tick) without the read handshake. If refused, call read_signoff, then retry. Never write memory-root state except through this server's tools.",
    ] {
        assert!(
            instructions.lines().any(|l| l == line),
            "§11 line must ride verbatim: {line}"
        );
    }
}

#[test]
fn newly_reachable_tools_refuse_before_handshake_in_the_pinned_envelope() {
    // The four wave-3 tools the registry now actually reaches (they were
    // implemented but never advertised/dispatchable). Each module runs
    // the uniform gate (spec §4) BEFORE argument validation, so even
    // empty arguments draw the exact pinned refusal — and spec §8's
    // both-halves rule pins BOTH the refusal shape AND the absent side
    // effect (a fresh root stays byte-empty).
    let mut s = fresh_server("newtools-refusal");
    assert!(
        std::fs::read_dir(&s.root).unwrap().next().is_none(),
        "a fresh configured root starts EMPTY"
    );
    for (id, name) in [
        (101u64, "claim_orchestrator"),
        (102, "write_dayfile"),
        (103, "write_brief"),
        (104, "write_warm_start"),
    ] {
        let frame = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"tools/call\",\"params\":{{\"name\":\"{name}\",\"arguments\":{{}}}}}}"
        );
        let out = rpc::handle_frame(&mut s, &frame).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v["error"].is_null(),
            "{name}: a refusal is a normal result, never a JSON-RPC error frame"
        );
        assert_eq!(v["id"], id, "{name}: the id must round-trip");
        assert_eq!(
            v["result"]["content"][0]["text"],
            "handshake incomplete — call read_signoff first — memory protocol: read_signoff is the first action of every session — call it, then retry. — if read_signoff itself reports the root unprovisioned, that is NOT a call-order problem: provision the root or report to the human (retrying cannot fix it).",
            "{name}: the refusal must carry the exact shared-gate rule text (gate::HANDSHAKE_REFUSAL_MESSAGE)"
        );
        assert_eq!(
            s.attempted_write_before_handshake.get(&s.session_id),
            Some(&(id - 100)),
            "{name}: the refused attempt must count at the gate"
        );
    }
    // Side effect ABSENT: every refusal happened before any disk touch —
    // the root is exactly as empty as it started.
    assert!(
        std::fs::read_dir(&s.root).unwrap().next().is_none(),
        "refused calls must leave the root exactly as empty as it started"
    );
}

#[test]
fn claim_orchestrator_success_arrives_as_one_serialized_text_item() {
    // The success half at the frame level for a newly reachable tool:
    // ONE text item holding the handler's serialized JSON, NO isError,
    // and no raw handler field leaking beside the envelope (the raw
    // HandlerResult shape lives at module level only).
    let mut s = fresh_server("newtools-claim");
    s.handshaken.insert(s.session_id.clone(), true);
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"claim_orchestrator","arguments":{}}}"#,
    )
    .unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["result"]["isError"].is_null(), "success carries no isError");
    let content = v["result"]["content"]
        .as_array()
        .expect("success carries a content array");
    assert_eq!(content.len(), 1, "exactly ONE text item");
    assert_eq!(content[0]["type"], "text");
    assert!(
        v["result"].get("claimed").is_none(),
        "raw handler fields must not leak beside the envelope"
    );
    let payload: Value = serde_json::from_str(content[0]["text"].as_str().unwrap())
        .expect("the text item is the serialized handler result");
    assert_eq!(payload["claimed"], true);
    assert!(payload["token"].as_str().unwrap().starts_with("tok-"));
    // The claim really landed on disk: the file is the truth (spec §5).
    let claim_files: Vec<String> = std::fs::read_dir(s.root.join(".claims"))
        .expect("the fresh claim wrote .claims/orchestrator-<date>")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("orchestrator-"))
        .collect();
    assert_eq!(claim_files.len(), 1, "exactly one claim file");
}
