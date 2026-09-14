//! totalrecall — single-binary MCP server entry point (spec §2, §12).
//!
//! Usage: `totalrecall [--root <path>] [--self-check]`
//!
//! `--self-check` resolves the memory root, runs the init-fingerprint
//! startup gate, prints ONE JSON line `{root, tz, tz_offset_minutes,
//! session}` to stdout, and exits 0. Without the flag the server serves
//! JSON-RPC 2.0 (newline-delimited) over stdio; all logs go to stderr.

use std::process::exit;

fn main() {
    let (cli_root, self_check) = match parse_args() {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("totalrecall: {msg}");
            eprintln!("usage: totalrecall [--root <path>] [--self-check]");
            exit(2);
        }
    };

    let args = totalrecall::config::ConfigArgs { cli_root };
    let root = match totalrecall::config::resolve_root(&args) {
        Ok(root) => root,
        Err(msg) => {
            eprintln!("totalrecall: refusing to start: {msg}");
            exit(1);
        }
    };

    if self_check {
        if let Err(msg) = totalrecall::config::init_state(&root) {
            eprintln!("totalrecall: self-check failed: {msg}");
            exit(1);
        }
        let (tz, offset) = match totalrecall::config::resolve_tz() {
            Ok(pair) => pair,
            Err(msg) => {
                eprintln!("totalrecall: self-check failed: {msg}");
                exit(1);
            }
        };
        let server = totalrecall::rpc::new_server(&root);
        // One JSON line on stdout, in the contract's field order.
        println!(
            "{{\"root\":{},\"tz\":{},\"tz_offset_minutes\":{},\"session\":{}}}",
            json_quote(&root.display().to_string()),
            json_quote(&tz),
            offset,
            json_quote(&server.session_id),
        );
        exit(0);
    }

    // Startup: the init fingerprint IS the loud gate (spec §2) — a
    // mixed-TZ or moved shared root refuses before anything is served.
    if let Err(msg) = totalrecall::config::init_state(&root) {
        eprintln!("totalrecall: refusing to start: {msg}");
        exit(1);
    }
    let code = totalrecall::rpc::serve_stdio(&root);
    exit(code);
}

/// Parses `totalrecall [--root <path>] [--self-check]` from argv.
///
/// The parsed `--root` goes into the lib's `ConfigArgs`; `--self-check`
/// is binary-only state (the contract pins `ConfigArgs` to one field,
/// and the gate constructs it by literal).
fn parse_args() -> Result<(Option<String>, bool), String> {
    let mut cli_root: Option<String> = None;
    let mut self_check = false;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--root" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--root requires a <path> argument".to_string())?;
                if cli_root.is_some() {
                    return Err("duplicate --root flag".to_string());
                }
                cli_root = Some(value);
            }
            "--self-check" => {
                if self_check {
                    return Err("duplicate --self-check flag".to_string());
                }
                self_check = true;
            }
            other => {
                return Err(format!("unknown argument {other:?}"));
            }
        }
    }
    Ok((cli_root, self_check))
}

/// Renders a string as a JSON string literal (quoted, escaped) so the
/// self-check line keeps the contract's field order (a `serde_json`
/// `Map` would re-sort keys alphabetically).
fn json_quote(s: &str) -> String {
    serde_json::Value::from(s.to_string()).to_string()
}
