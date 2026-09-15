//! §8 gate — wave 1 slice (config + rpc contract).
//!
//! ORCHESTRATOR-OWNED per ops/parallel-workers.md invariant 5: written BEFORE the
//! wave launches, workers MUST NOT edit this file. It fails loudly (compile error)
//! until the wave's modules land — that is the intended loud failure.
//!
//! Contract being asserted (see local://exo-mcp-build-contract.md §API):
//! - `config::resolve_root(&ConfigArgs)` precedence flag > EXOMEMORY_DIR > EXO_DIR
//!   > config file > default; both env names differing -> Err (loud).
//! - `config::init_state(&Path)` writes `.state/init.json` {root, tz, tz_offset_minutes,
//!   created_at}; a later init on a different zone/offset for the same root -> Err.
//! - `rpc::handle_frame` JSON-RPC 2.0: initialize negotiates 2026-07-28 (and any
//!   earlier revision the server implements); unknown tool name -> JSON-RPC error
//!   -32602; malformed frame -> -32700; gated-tool refusal -> normal result with
//!   isError:true + content array (never a JSON-RPC error object).
//! - Fresh configured root starts EMPTY: only `.state/` exists after init.

use total_recall::config::{self, ConfigArgs};
use total_recall::rpc::{self, Server};
use serde_json::json;

use std::sync::atomic::{AtomicUsize, Ordering};
static ENV_SPIN: AtomicUsize = AtomicUsize::new(0);
fn env_acquire() {
    while ENV_SPIN.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        std::thread::yield_now();
    }
}
fn env_release() {
    ENV_SPIN.store(0, Ordering::Release);
}
fn tmp_root(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("total-recall-gate-w1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn resolve_root_flag_beats_env() {
    env_acquire();
    let dir = tmp_root("flag");
    let cfg = config::resolve_root(&ConfigArgs {
        cli_root: Some(dir.display().to_string()),
    })
    .unwrap();
    assert_eq!(cfg, dir, "flag must beat env/config/default");
    env_release();
}

#[test]
fn resolve_root_env_beats_config_file_and_default() {
    env_acquire();
    let dir = tmp_root("env");
    // EXOMEMORY_DIR wins over any config-file/default answer.
    std::env::set_var("EXOMEMORY_DIR", dir.display().to_string());
    std::env::remove_var("EXO_DIR");
    let cfg = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap();
    assert_eq!(cfg, dir, "env must beat config file and default");
    std::env::remove_var("EXOMEMORY_DIR");
    env_release();
}

#[test]
fn resolve_root_two_env_names_differing_is_loud() {
    env_acquire();
    let a = tmp_root("env-a");
    let b = tmp_root("env-b");
    std::env::set_var("EXOMEMORY_DIR", a.display().to_string());
    std::env::set_var("EXO_DIR", b.display().to_string());
    let err = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap_err();
    assert!(!err.is_empty(), "both env names differing must refuse loudly");
    std::env::remove_var("EXOMEMORY_DIR");
    std::env::remove_var("EXO_DIR");
    env_release();
}

#[test]
fn init_state_writes_fingerprint_and_refuses_tz_change() {
    let dir = tmp_root("tz");
    config::init_state(&dir).unwrap();
    let st = dir.join(".state/init.json");
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&st).unwrap())
        .expect(".state/init.json must be valid JSON");
    assert_eq!(v["root"], dir.display().to_string());
    assert!(v["tz"].is_string() && !v["tz"].as_str().unwrap().is_empty());
    assert!(v["tz_offset_minutes"].is_i64());
    assert!(v["created_at"].is_string());

    // A later start that sees a DIFFERENT zone/offset for the same root refuses loudly.
    let mut forged = v.clone();
    forged["tz"] = json!("UTC");
    forged["tz_offset_minutes"] = json!(0);
    std::fs::write(&st, forged.to_string()).unwrap();
    let err = config::init_state(&dir).unwrap_err();
    assert!(
        err.contains("timezone") || err.contains("zone"),
        "mismatch must be loud: {err}"
    );
}

#[test]
fn fresh_root_starts_empty() {
    let dir = tmp_root("fresh");
    config::init_state(&dir).unwrap();
    assert!(!dir.join("signoff.md").exists());
    assert!(!dir.join("2026-09-13.md").exists());
    assert!(!dir.join(".claims").exists());
    assert!(!dir.join(".audit").exists());
    assert!(!dir.join(".locks").exists());
    assert!(dir.join(".state").exists(), "only .state exists after init");
}

#[test]
fn rpc_initialize_negotiates_current_revision() {
    let dir = tmp_root("rpc-init");
    config::init_state(&dir).unwrap();
    let mut s = Server::new_server(&dir);
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["result"]["protocolVersion"], "2026-07-28");
    assert!(v["result"]["capabilities"]["tools"]["listChanged"].is_boolean());
}

#[test]
fn rpc_unknown_tool_name_is_32602_not_32601() {
    let dir = tmp_root("rpc-tool");
    config::init_state(&dir).unwrap();
    let mut s = Server::new_server(&dir);
    // tools/call with an unknown tool name -> JSON-RPC error -32602 (Invalid params).
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"no_such_tool","arguments":{}}}"#,
    )
    .unwrap_err();
    assert!(out.contains("-32602"), "unknown tool must be -32602: {out}");
    // Same for tools/list with an unknown method: -32601 (method not found).
    let out2 = rpc::handle_frame(&mut s, r#"{"jsonrpc":"2.0","id":3,"method":"tools/nope","params":{}}"#)
        .unwrap_err();
    assert!(out2.contains("-32601"), "unknown method must be -32601: {out2}");
}

#[test]
fn rpc_malformed_frame_is_32700_parse_error() {
    let dir = tmp_root("rpc-parse");
    config::init_state(&dir).unwrap();
    let mut s = Server::new_server(&dir);
    let out = rpc::handle_frame(&mut s, "{not json").unwrap_err();
    assert!(out.contains("-32700"), "malformed frame must be -32700: {out}");
}

#[test]
fn rpc_refusal_is_result_with_iserror_no_side_effect() {
    // A gated handler must return a normal result carrying isError:true + content,
    // never a JSON-RPC error object — and the file must be unchanged.
    let dir = tmp_root("rpc-refusal");
    config::init_state(&dir).unwrap();
    let mut s = Server::new_server(&dir);
    // append_signoff is gated: without the handshake it refuses and writes nothing.
    let out = rpc::handle_frame(
        &mut s,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"append_signoff","arguments":{"role":"probe","workflow":"w","done":"yes"}}}"#,
    )
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["result"]["isError"].as_bool().unwrap(), "refusal must be isError:true");
    assert!(v["result"]["content"].is_array());
    assert!(v["error"].is_null(), "refusal must NOT be a JSON-RPC error object");
    assert!(
        !dir.join("signoff.md").exists(),
        "side effect must be ABSENT: no signoff.md created"
    );
}
