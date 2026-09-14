//! JSON-RPC 2.0 / MCP protocol layer over stdio (spec §2).
//!
//! Wave 1 pinned the protocol SEMANTICS: initialize negotiation,
//! tools/list, tools/call dispatch, the error codes, notification
//! silence, and the stdio frame loop. Wave 2 merges the tool registry:
//! the tool shapes and dispatch entries are owned by the tool modules
//! (`crate::tools::signoff_read`, `crate::tools::signoff_append`), and
//! `new_server` assembles their `tools()`/`handlers()` entries, adding
//! nothing of its own. The protocol layer wraps NO handler in
//! `crate::gate::gate` — gated handlers (e.g. `append_signoff`) run
//! the gate check and count refused attempts INLINE, and a second wrap
//! would tick `attempted_write_before_handshake` twice (gate_w3 pins
//! the exact count). A refused handler is still rendered as a NORMAL
//! result carrying `isError: true` + a `content` array (spec §2/§4) —
//! never a JSON-RPC `error` object.
//!
//! Pinned semantics (build contract §src/rpc.rs; spec §2 protocol shapes):
//! - malformed JSON                -> Err frame, code -32700 "parse error"
//! - valid JSON, invalid request   -> Err frame, code -32600 "invalid request"
//! - unknown method                 -> Err frame, code -32601
//! - unknown tool name (tools/call) -> Err frame, code -32602
//! - handler refusal                 -> Ok frame, result {isError:true, content:[…]}
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
/// `Ok(value)` becomes the JSON-RPC `result`. `Err(msg)` becomes a NORMAL
/// result carrying `{isError: true, content: [{type:"text", text: msg}]}`
/// — a refusal is a tool-level outcome, never a JSON-RPC `error` object
/// (spec §2: "Tool execution failure … -> a normal JSON-RPC result").
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
        // The merged registry (wave 2): the tool modules own the tool
        // shapes and the dispatch entries; the protocol layer only
        // assembles them, in handshake order (read_signoff first, then
        // append_signoff). `append_signoff`'s handler runs the gate
        // check and counts refused attempts INLINE (see the module
        // docs) — do NOT additionally wrap that entry in
        // `crate::gate::gate`, which would double-count every refusal.
        let mut tools = crate::tools::signoff_read::tools();
        tools.extend(crate::tools::signoff_append::tools());
        let mut handlers = crate::tools::signoff_read::handlers();
        for (name, handler) in crate::tools::signoff_append::handlers() {
            handlers.insert(name, handler);
        }
        Server {
            root: root.to_path_buf(),
            session_id: generate_session_id(),
            handshaken: HashMap::new(),
            tools,
            handlers,
            attempted_write_before_handshake: HashMap::new(),
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
                            HandlerResult::Ok(result) => Ok(result),
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
    })
}

/// `tools/list`: advertise the merged registry — the tool modules'
/// shapes (the handshake pair: the ungated `read_signoff` gate-opener,
/// then the gated `append_signoff`).
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
        "[exomem-mcp] session {} — JSON-RPC 2.0 over stdio (root: {})",
        server.session_id,
        server.root.display()
    );

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("[exomem-mcp] stdin read error: {err}");
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
                        eprintln!("[exomem-mcp] stdout write error: {err}");
                        return 1;
                    }
                }
            }
            Err(frame) => {
                eprintln!("[exomem-mcp] protocol error frame: {frame}");
                if let Err(err) = writeln!(std::io::stdout(), "{frame}") {
                    eprintln!("[exomem-mcp] stdout write error: {err}");
                    return 1;
                }
            }
        }
    }
    0
}
