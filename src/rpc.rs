//! JSON-RPC 2.0 / MCP protocol layer over stdio (spec §2).
//!
//! Wave 1 pinned the protocol SEMANTICS: initialize negotiation,
//! tools/list, tools/call dispatch, the error codes, notification
//! silence, and the stdio frame loop. The registry is merged: the tool
//! shapes and dispatch entries are owned by the tool modules
//! (`crate::tools::signoff_read`, `crate::tools::signoff_append`,
//! `crate::tools::claims`, `crate::tools::briefs`,
//! `crate::tools::warmstart` — wave 4 appends its modules the same
//! way), and `new_server` assembles their `tools()`/`handlers()`
//! entries, adding nothing of its own. The protocol layer wraps NO
//! handler in `crate::gate::gate` — gated handlers (e.g.
//! `append_signoff`) run the gate check and count refused attempts
//! INLINE, and a second wrap would tick
//! `attempted_write_before_handshake` twice (gate_w3 pins the exact
//! count). A refused handler is still rendered as a NORMAL result
//! carrying `isError: true` + a `content` array (spec §2/§4) — never a
//! JSON-RPC `error` object.
//!
//! Pinned semantics (build contract §src/rpc.rs; spec §2 protocol shapes):
//! - malformed JSON                -> Err frame, code -32700 "parse error"
//! - valid JSON, invalid request   -> Err frame, code -32600 "invalid request"
//! - unknown method                 -> Err frame, code -32601
//! - unknown tool name (tools/call) -> Err frame, code -32602
//! - handler success                 -> Ok frame, result {content:[{type:"text", text:<serialized handler JSON>}]} — no isError
//! - handler refusal                 -> Ok frame, result {isError:true, content:[…]}
//! - unserializable handler value    -> Err frame, code -32603 (server bug, fails loud)
//! - notification (no `id` member)   -> Ok("") — nothing is ever emitted
//!
//! stdout carries protocol frames only (newline-delimited); diagnostics go
//! to stderr. One server process per top-level client session: identity is
//! process-local (spec §2 — the harness exports no session-id env var).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

use std::path::Path;

/// Current MCP specification revision (fetched 2026-09-13, spec §2).
const PROTOCOL_VERSION: &str = "2026-07-28";
/// Earlier revision the server also implements; a client requesting it is
/// echoed back (spec §2: "the server also accepts earlier revisions it
/// implements"). Any other requested revision is answered with the current.
const PROTOCOL_VERSION_PREV: &str = "2025-11-25";

/// The server: one process-local session, the tool registry, and the
/// per-session gate state (spec §4).
///
/// `handshaken` is PROCESS-LOCAL by construction: a restart means a
/// re-handshake. Claims are deliberately NOT stored here — the on-disk
/// claim files under `.claims/` are the source of truth (spec §5).
pub struct Server {
    /// The configured memory root (resolved before the server is built).
    pub root: PathBuf,
    /// Process-local session identity, `s-<8 hex>`. Generated at
    /// construction; persisted to `.state/` at handshake (a later-wave
    /// `read_signoff` performs the write).
    pub session_id: String,
    /// session_id -> handshaken. The uniform gate (spec §4) consults this
    /// map for every disk-touching call.
    pub handshaken: HashMap<String, bool>,
    /// The tool registry advertised by `tools/list` (spec §3 shapes).
    pub tools: Vec<Tool>,
    /// method name -> dispatch entry point for `tools/call`.
    pub handlers: HashMap<String, Handler>,
    /// session_id -> claim token for the date the session claims (spec §2:
    /// "Server keeps per-session handshake state `{session_id, handshaked,
    /// claim_token}`"). `claim_orchestrator` stores the token here per-session;
    /// `write_brief` validates ONLY this field (its schema carries no token
    /// param), while `write_dayfile` / `write_warm_start` validate BOTH this
    /// field and the caller-supplied token against the claim file's token for
    /// the LOCAL date (spec §5). Process-local: a server restart re-stores it
    /// via re-claim (the claim file is the disk truth, §5).
    pub claim_tokens: HashMap<String, String>,
    /// session_id -> count of gated-tool calls ATTEMPTED and refused
    /// before the handshake. This is what makes the `session_compliance`
    /// metric "observable at the gate" (spec §4).
    pub attempted_write_before_handshake: HashMap<String, u64>,
}

/// One advertised tool: name, the MANDATORY contract text carried in
/// `description` (spec §4 — "tool descriptions carry the MANDATORY
/// contract text"), and the pinned input schema (spec §3).
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A dispatch entry point: a non-capturing function pointer (no
/// allocation per call, no per-session state smuggled past the `Server`).
pub type Handler = fn(&mut Server, &Value) -> HandlerResult;

/// What a tool handler reports back to the frame loop.
///
/// `Ok(value)` is wrapped at the tools/call dispatch point ONLY into the
/// MCP-mandatory success envelope
/// `{content: [{type:"text", text: serde_json::to_string(value)}]}` — no
/// `isError` on success (the handlers themselves return these raw values
/// UNCHANGED; their module-level tests depend on that). `Err(msg)` becomes
/// a NORMAL result carrying `{isError: true, content: [{type:"text",
/// text: msg}]}` — a refusal is a tool-level outcome, never a JSON-RPC
/// `error` object (spec §2: "Tool execution failure … -> a normal
/// JSON-RPC result").
pub enum HandlerResult {
    Ok(Value),
    Err(String),
}

impl Server {
    /// Build a server for the resolved root. Side-effect-free: no bucket is
    /// created or read (a fresh configured root starts EMPTY — only
    /// `config::init_state` touches the disk, and it creates `.state/`
    /// alone).
    pub fn new_server(root: &Path) -> Server {
        // The merged registry: every tool module owns its tool shapes and
        // its dispatch entries; the protocol layer only assembles the
        // modules listed here — wiring another module (wave 4: recall,
        // log_tick/last_tick, session_compliance) is ONE symmetric row of
        // its `tools()` + `handlers()`. Order is the handshake order: the
        // ungated gate-opener `read_signoff` is advertised first.
        // Gated handlers run the gate check and count refused attempts
        // INLINE (see the module docs) — do NOT additionally wrap these
        // entries in `crate::gate::gate`, which would double-count every
        // refusal.
        let modules: Vec<(Vec<Tool>, HashMap<String, Handler>)> = vec![
            (
                crate::tools::signoff_read::tools(),
                crate::tools::signoff_read::handlers(),
            ),
            (
                crate::tools::signoff_append::tools(),
                crate::tools::signoff_append::handlers(),
            ),
            (crate::tools::claims::tools(), crate::tools::claims::handlers()),
            (crate::tools::briefs::tools(), crate::tools::briefs::handlers()),
            (
                crate::tools::warmstart::tools(),
                crate::tools::warmstart::handlers(),
            ),
            (crate::tools::recall::tools(), crate::tools::recall::handlers()),
            (crate::tools::ticks::tools(), crate::tools::ticks::handlers()),
            (
                crate::tools::compliance::tools(),
                crate::tools::compliance::handlers(),
            ),
        ];
        let mut tools = Vec::new();
        let mut handlers: HashMap<String, Handler> = HashMap::new();
        for (module_tools, module_handlers) in modules {
            tools.extend(module_tools);
            handlers.extend(module_handlers);
        }
        Server {
            root: root.to_path_buf(),
            session_id: generate_session_id(),
            handshaken: HashMap::new(),
            tools,
            handlers,
            attempted_write_before_handshake: HashMap::new(),
            claim_tokens: HashMap::new(),
        }
    }
}

/// Module-level builder, per the build contract; delegates to
/// [`Server::new_server`] (the form the gate tests call).
pub fn new_server(root: &Path) -> Server {
    Server::new_server(root)
}

/// Generate the process-local session id: `s-` + 8 hex digits, mixed from
/// the pid and a monotonic clock reading — unique per process start without
/// any entropy dependency (Rust std + serde_json only). Matches the
/// reference format in spec §3 (`s-7c1f9d2e`).
fn generate_session_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = ((std::process::id() as u64).wrapping_mul(0x9E37_79B9) ^ nanos) as u32;
    format!("s-{mixed:08x}")
}

/// Dispatch one newline-delimited JSON-RPC 2.0 frame.
///
/// `Ok(frame)` is a response frame to write to stdout — except the empty
/// string, which means "notification: emit nothing". `Err(frame)` is a
/// JSON-RPC error frame (parse / invalid-request / unknown method / unknown
/// tool) that must STILL be written to stdout: an error frame is a
/// response, and a client that sent a malformed line must hear about it.
pub fn handle_frame(server: &mut Server, frame: &str) -> Result<String, String> {
    // Malformed: not valid JSON at all -> -32700 parse error, id null
    // (the id is unrecoverable), per the pinned contract frame.
    let request: Value = match serde_json::from_str(frame) {
        Ok(value) => value,
        Err(_) => {
            return Err(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"parse error"}})
                    .to_string(),
            )
        }
    };

    // Structurally invalid: valid JSON that is not a JSON-RPC request
    // (not an object, or no usable `method` member) -> -32600 invalid
    // request. A named-but-unrecognized method is a different failure and
    // gets -32601 below.
    let members = match request.as_object() {
        Some(members) => members,
        None => {
            return Err(invalid_request_frame(null_id(), "invalid request: not a JSON object"))
        }
    };
    let method = match members.get("method").and_then(|value| value.as_str()) {
        Some(method) => method.to_string(),
        None => {
            return Err(invalid_request_frame(
                members.get("id").cloned().unwrap_or(Value::Null),
                "invalid request: missing or non-string method",
            ))
        }
    };

    // Notification: a request with NO `id` member. The server MUST NOT emit
    // any response for it — including for a method it does not implement.
    let id = match members.get("id") {
        Some(id) => id.clone(),
        None => {
            eprintln!("[rpc] notification {method} (no response emitted)");
            return Ok(String::new());
        }
    };

    let outcome: Result<Value, (i64, &str, String)> = match method.as_str() {
        "initialize" => Ok(handle_initialize(&request)),
        "tools/list" => Ok(tools_list_result(server)),
        "tools/call" => {
            let name = members
                .get("params")
                .and_then(|params| params.get("name"))
                .and_then(|value| value.as_str());
            match name {
                // No usable tool name (or a name the registry does not
                // hold) -> -32602 Invalid params. Never -32601: that code
                // names an unknown METHOD, not an unknown tool (spec §2).
                Some(name) => match server.handlers.get(name) {
                    None => Err((-32602, "unknown tool", format!("unknown tool name: {name}"))),
                    Some(handler) => {
                        let arguments = members
                            .get("params")
                            .and_then(|params| params.get("arguments").cloned())
                            .unwrap_or_else(|| json!({}));
                        match handler(server, &arguments) {
                            // Success: MCP mandates a `content` array on
                            // every tools/call result — the handler's raw
                            // JSON value travels as ONE text item holding
                            // its serialization. The envelope is built
                            // HERE and only here; handlers return raw
                            // values. No `isError` on success.
                            HandlerResult::Ok(result) => match serde_json::to_string(&result) {
                                Ok(text) => Ok(json!({
                                    "content": [{"type": "text", "text": text}],
                                })),
                                // A value that cannot serialize is a
                                // server bug: fail loud with an
                                // internal-error frame, never a half-built
                                // success envelope.
                                Err(err) => Err((
                                    -32603,
                                    "internal error",
                                    format!("tools/call: result serialization failed for {name}: {err}"),
                                )),
                            },
                            // Refusal: a normal result, isError + content
                            // array — never a JSON-RPC error object.
                            HandlerResult::Err(message) => Ok(json!({
                                "isError": true,
                                "content": [{"type": "text", "text": message}],
                            })),
                        }
                    }
                },
                None => Err((-32602, "unknown tool", "unknown tool name: params.name missing".to_string())),
            }
        }
        // Every other named method this server does not implement.
        _ => Err((-32601, "method not found", format!("unknown method: {method}"))),
    };

    match outcome {
        Ok(result) => Ok(json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()),
        Err((code, _label, message)) => Err(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}).to_string(),
        ),
    }
}

fn null_id() -> Value {
    Value::Null
}

fn invalid_request_frame(id: Value, message: &str) -> String {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":message}}).to_string()
}

/// Identity the server reports in the `initialize` result (MCP requires
/// `serverInfo`): the bin name, versioned with spec.md (v0.3).
const SERVER_NAME: &str = "totalrecall";
const SERVER_VERSION: &str = "0.3.0";

/// The spec §11 prompt layer — "the only prompt that should exist" —
/// delivered VERBATIM on the MCP-native channel: omp's client reads
/// `instructions` off the initialize result and injects it into the
/// model-facing system prompt per connected server. The tool names are
/// the harness-generated surface names (`mcp__<server>_<tool>`); the
/// server registers as `totalrecall`, so they resolve.
/// Byte-equal to the spec §11 fenced block INCLUDING its trailing
/// newline (FOUND-4): a client byte-diffing the field against the spec
/// text must find zero difference.
const SERVER_INSTRUCTIONS: &str = "## Memory protocol (MCP: totalrecall)\n- First action of every session: mcp__totalrecall_read_signoff. No work before it returns.\n- Last action before any stop/compact/handoff: mcp__totalrecall_append_signoff {role, workflow, done, unpushed, awaits_human, still_running, kaibo_review}.\n- Facts come from mcp__totalrecall_recall results (dated) or files — never from your context memory.\n- The server refuses every append path (including log_tick) without the read handshake. If refused, call read_signoff, then retry. Never write memory-root state except through this server's tools.\n";

/// `initialize`: negotiate the protocol revision. The server answers with
/// the requested revision when it implements it (the current `2026-07-28`,
/// or the earlier `2025-11-25`); anything else — unknown, future, or
/// missing — is answered with the current revision. initialize is protocol
/// negotiation, NOT the session handshake: it never touches `handshaken`
/// (spec §4 grants the handshake to `read_signoff` only).
fn handle_initialize(request: &Value) -> Value {
    let requested = request
        .get("params")
        .and_then(|params| params.get("protocolVersion"))
        .and_then(|value| value.as_str());
    let protocol_version = match requested {
        Some(PROTOCOL_VERSION) | Some(PROTOCOL_VERSION_PREV) => requested.unwrap(),
        _ => PROTOCOL_VERSION,
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
        "instructions": SERVER_INSTRUCTIONS,
    })
}

/// `tools/list`: advertise the merged registry — every tool module's
/// shapes, in handshake order (the ungated `read_signoff` gate-opener
/// first; see `Server::new_server` for the module rows).
fn tools_list_result(server: &Server) -> Value {
    let tools: Vec<Value> = server
        .tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    json!({"tools": tools})
}

/// The stdio loop (spec §2: newline-delimited JSON-RPC over stdio, logs to
/// stderr). Reads lines until EOF; writes every non-empty response frame to
/// stdout (error frames included — a client that sent garbage must hear
/// about it); notifications produce no output at all. Returns 0 on clean
/// EOF.
pub fn serve_stdio(root: &Path) -> i32 {
    let mut server = Server::new_server(root);
    eprintln!(
        "[totalrecall] session {} — JSON-RPC 2.0 over stdio (root: {})",
        server.session_id,
        server.root.display()
    );

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("[totalrecall] stdin read error: {err}");
                return 1;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match handle_frame(&mut server, &line) {
            Ok(frame) => {
                if !frame.is_empty() {
                    if let Err(err) = writeln!(std::io::stdout(), "{frame}") {
                        eprintln!("[totalrecall] stdout write error: {err}");
                        return 1;
                    }
                }
            }
            Err(frame) => {
                eprintln!("[totalrecall] protocol error frame: {frame}");
                if let Err(err) = writeln!(std::io::stdout(), "{frame}") {
                    eprintln!("[totalrecall] stdout write error: {err}");
                    return 1;
                }
            }
        }
    }
    0
}
