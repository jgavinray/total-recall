//! `total-recall-mcp` — install/print MCP configuration for omp, Pi,
//! Claude Code, and Codex.
//!
//! The server is a stdio MCP process; each harness spawns it from its own
//! configuration file. This helper writes the same entry into every
//! supported harness, provisions the memory root, and is idempotent. It
//! is deliberately a separate binary: the server's MCP contract (§2/§3)
//! is unchanged, and a harness config is not an MCP tool.
//!
//! Supported files:
//! - omp:   `~/.omp/agent/mcp.json`        (`mcpServers`, stdio)
//! - Pi:    `~/.pi/agent/mcp.json`         (`mcpServers`, stdio)
//! - Claude: `~/.claude.json`              (top-level `mcpServers`, user scope)
//! - Codex: `~/.codex/config.toml`         (`[mcp_servers.total-recall]`)
//!
//! Usage:
//!   total-recall-mcp install [--harness all|omp,pi,claude,codex]
//!                            [--root PATH] [--binary PATH] [--dry-run] [--force]
//!                            [--omp-config PATH] [--pi-config PATH]
//!                            [--claude-config PATH] [--codex-config PATH]
//!   total-recall-mcp print   [same options]
//!
//! `install` creates the root when absent, seeds `signoff.md` when absent,
//! runs the server's init fingerprint, and writes each harness config.
//! `print` emits the snippets without touching disk.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SERVER_NAME: &str = "total-recall";
const OMP_SCHEMA: &str =
    "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/config/mcp-schema.json";
const OMP_TIMEOUT_MS: u64 = 60_000;
const PI_TIMEOUT_SECS: u64 = 60;
const CODEX_STARTUP_SECS: u64 = 10;
const CODEX_TOOL_SECS: u64 = 60;
const PI_DESCRIPTION: &str =
    "Enforced markdown memory: warm start, day file, briefs, signoffs, recall, ticks";

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Harness {
    Omp,
    Pi,
    Claude,
    Codex,
}

impl Harness {
    fn label(self) -> &'static str {
        match self {
            Harness::Omp => "omp",
            Harness::Pi => "pi",
            Harness::Claude => "claude",
            Harness::Codex => "codex",
        }
    }

    fn all() -> [Harness; 4] {
        [Harness::Omp, Harness::Pi, Harness::Claude, Harness::Codex]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Install,
    Print,
}

#[derive(Debug, Default)]
struct Options {
    action: Option<Action>,
    harnesses: Vec<Harness>,
    root: Option<String>,
    binary: Option<String>,
    dry_run: bool,
    force: bool,
    omp_config: Option<PathBuf>,
    pi_config: Option<PathBuf>,
    claude_config: Option<PathBuf>,
    codex_config: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Created,
    Updated,
    Unchanged,
}

fn main() {
    let opts = match parse_args() {
        Ok(opts) => opts,
        Err(msg) => {
            eprintln!("total-recall-mcp: {msg}");
            eprintln!("usage: total-recall-mcp install|print [--harness all|omp,pi,claude,codex] [--root PATH] [--binary PATH] [--dry-run] [--force]");
            std::process::exit(2);
        }
    };

    if let Err(msg) = run(&opts) {
        eprintln!("total-recall-mcp: {msg}");
        std::process::exit(1);
    }
}

fn run(opts: &Options) -> Result<(), String> {
    let action = opts.action.unwrap_or(Action::Install);
    let harnesses = if opts.harnesses.is_empty() {
        Harness::all().to_vec()
    } else {
        opts.harnesses.clone()
    };

    let root = resolve_root(opts)?;
    let binary = resolve_binary(opts.binary.as_deref())?;

    match action {
        Action::Print => {
            print_snippets(&harnesses, &binary, &root, opts)?;
            Ok(())
        }
        Action::Install => install(&harnesses, &binary, &root, opts),
    }
}

fn install(
    harnesses: &[Harness],
    binary: &Path,
    root: &Path,
    opts: &Options,
) -> Result<(), String> {
    // The server's own startup gate is the authority for the root's
    // timezone fingerprint. Running it before seeding also creates
    // `.state/` so `claims::local_date` uses the recorded offset.
    // A dry run touches no disk at all.
    if opts.dry_run {
        println!("would provision root {}", root.display());
        check_env_root(root, opts.force)?;
    } else {
        total_recall::config::init_state(root).map_err(|e| format!("root init failed: {e}"))?;
        check_env_root(root, opts.force)?;
        let seeded = seed_signoff(root)?;
        if seeded {
            println!("root: created {}/signoff.md", root.display());
        } else {
            println!("root: signoff.md already present at {}", root.display());
        }
    }

    for harness in harnesses {
        let path = config_path(*harness, opts)?;
        let change = match harness {
            Harness::Codex => {
                let block = codex_block(binary, root);
                upsert_codex(&path, &block, opts.dry_run)?
            }
            Harness::Omp => {
                let entry = omp_entry(binary, root);
                upsert_json(&path, &entry, true, opts.dry_run)?
            }
            Harness::Pi => {
                let entry = pi_entry(binary, root);
                upsert_json(&path, &entry, false, opts.dry_run)?
            }
            Harness::Claude => {
                let entry = claude_entry(binary, root);
                upsert_json(&path, &entry, false, opts.dry_run)?
            }
        };
        println!(
            "{}: {} {}",
            harness.label(),
            change_word(change, opts.dry_run),
            path.display()
        );
    }

    println!();
    println!("Restart each harness so it reloads MCP configuration.");
    println!("First call in every session: read_signoff.");
    println!("  omp:           mcp__total_recall_read_signoff");
    println!("  pi / Claude / Codex: the harness's generated name, typically");
    println!("                 mcp__total_recall__read_signoff");
    println!(
        "Set EXOMEMORY_DIR to {} so the omp guard and the server agree.",
        root.display()
    );
    Ok(())
}

fn print_snippets(
    harnesses: &[Harness],
    binary: &Path,
    root: &Path,
    opts: &Options,
) -> Result<(), String> {
    for harness in harnesses {
        let path = config_path(*harness, opts)?;
        println!("# {}", path.display());
        match harness {
            Harness::Codex => {
                print!("{}", codex_block(binary, root));
            }
            Harness::Omp => {
                let entry = omp_entry(binary, root);
                let doc = json!({
                    "$schema": OMP_SCHEMA,
                    "mcpServers": { SERVER_NAME: entry },
                });
                println!("{}", serde_json::to_string_pretty(&doc).unwrap());
            }
            Harness::Pi => {
                let entry = pi_entry(binary, root);
                let doc = json!({"mcpServers": { SERVER_NAME: entry }});
                println!("{}", serde_json::to_string_pretty(&doc).unwrap());
            }
            Harness::Claude => {
                let entry = claude_entry(binary, root);
                println!("# Merge this into the top-level mcpServers object of ~/.claude.json");
                let doc = json!({"mcpServers": { SERVER_NAME: entry }});
                println!("{}", serde_json::to_string_pretty(&doc).unwrap());
            }
        }
        println!();
    }
    println!("# Restart the harness, then call read_signoff first in every session.");
    println!("# omp: mcp__total_recall_read_signoff; other harnesses: typically");
    println!("# mcp__total_recall__read_signoff.");
    Ok(())
}

fn omp_entry(binary: &Path, root: &Path) -> Value {
    json!({
        "type": "stdio",
        "command": binary.display().to_string(),
        "args": ["--root", root.display().to_string()],
        "timeout": OMP_TIMEOUT_MS,
        "enabled": true
    })
}

fn pi_entry(binary: &Path, root: &Path) -> Value {
    json!({
        "command": binary.display().to_string(),
        "args": ["--root", root.display().to_string()],
        "timeout": PI_TIMEOUT_SECS,
        "enabled": true,
        "exposure": "direct",
        "description": PI_DESCRIPTION
    })
}

fn claude_entry(binary: &Path, root: &Path) -> Value {
    json!({
        "type": "stdio",
        "command": binary.display().to_string(),
        "args": ["--root", root.display().to_string()]
    })
}

fn codex_block(binary: &Path, root: &Path) -> String {
    format!(
        "[mcp_servers.{SERVER_NAME}]\ncommand = {}\nargs = [{}, {}]\nstartup_timeout_sec = {}\ntool_timeout_sec = {}\nenabled = true\n",
        toml_string(&binary.display().to_string()),
        toml_string("--root"),
        toml_string(&root.display().to_string()),
        CODEX_STARTUP_SECS,
        CODEX_TOOL_SECS
    )
}

fn config_path(harness: Harness, opts: &Options) -> Result<PathBuf, String> {
    let override_path = match harness {
        Harness::Omp => opts.omp_config.clone(),
        Harness::Pi => opts.pi_config.clone(),
        Harness::Claude => opts.claude_config.clone(),
        Harness::Codex => opts.codex_config.clone(),
    };
    if let Some(path) = override_path {
        return absolute(path);
    }
    let home = home_dir().ok_or_else(|| "cannot locate $HOME".to_string())?;
    Ok(match harness {
        Harness::Omp => home.join(".omp/agent/mcp.json"),
        Harness::Pi => home.join(".pi/agent/mcp.json"),
        Harness::Claude => home.join(".claude.json"),
        Harness::Codex => home.join(".codex/config.toml"),
    })
}

fn resolve_root(opts: &Options) -> Result<PathBuf, String> {
    let args = total_recall::config::ConfigArgs {
        cli_root: opts.root.clone(),
    };
    let root = total_recall::config::resolve_root(&args)?;
    absolute(root)
}

fn resolve_binary(explicit: Option<&str>) -> Result<PathBuf, String> {
    if let Some(raw) = explicit {
        let path = absolute(PathBuf::from(raw))?;
        if !path.is_file() {
            return Err(format!("--binary {} is not a file", path.display()));
        }
        return Ok(path);
    }

    // The helper is installed beside the server by `cargo install` and
    // Homebrew; prefer that sibling, then PATH.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("total-recall");
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("total-recall");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err("cannot locate the total-recall binary; pass --binary <path>".to_string())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn absolute(path: PathBuf) -> Result<PathBuf, String> {
    let joined = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| format!("cannot resolve relative path: {e}"))?
    };
    // Resolve `./` and symlinks when the path exists; keep the lexical
    // path otherwise (a config or root may be created later).
    Ok(joined.canonicalize().unwrap_or(joined))
}

/// Refuse a `--root` that disagrees with the guard's `EXOMEMORY_DIR`
/// (spec §6's two-plane agreement). `--force` is the explicit override.
fn check_env_root(root: &Path, force: bool) -> Result<(), String> {
    let Ok(raw) = std::env::var("EXOMEMORY_DIR") else {
        return Ok(());
    };
    if raw.trim().is_empty() {
        return Ok(());
    }
    let env_path = PathBuf::from(raw.trim());
    let env_path = env_path.canonicalize().unwrap_or(env_path);
    let root_path = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if root_path != env_path && !force {
        return Err(format!(
            "--root {} disagrees with EXOMEMORY_DIR {} — the omp guard and the server must share one root (spec §6); set both to the same path or pass --force",
            root_path.display(),
            env_path.display()
        ));
    }
    Ok(())
}

/// Seed `signoff.md` only when absent. The seed puts a `## Worker
/// sessions` heading after the warm block so `append_signoff`'s EOF
/// append lands outside the warm region (a warm block that is the last
/// section is rewritten by `write_warm_start`).
fn seed_signoff(root: &Path) -> Result<bool, String> {
    let path = root.join("signoff.md");
    if path.exists() {
        return Ok(false);
    }
    let date = total_recall::tools::claims::local_date(root)?;
    let content = format!(
        "# Signoff\n\nLast updated: {date} — root provisioned by total-recall-mcp.\n\n## If you read nothing else\n1. Nothing is in flight. Read this, work, sign off before you stop.\n\n## Worker sessions\n"
    );
    fs::write(&path, content).map_err(|e| format!("cannot seed {}: {e}", path.display()))?;
    Ok(true)
}

fn upsert_json(
    path: &Path,
    entry: &Value,
    add_omp_schema: bool,
    dry_run: bool,
) -> Result<Change, String> {
    let existed = path.exists();
    let mut doc = if existed {
        let text =
            fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?
    } else {
        json!({})
    };

    let obj = doc
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", path.display()))?;

    if add_omp_schema && obj.get("$schema").is_none() {
        obj.insert("$schema".to_string(), json!(OMP_SCHEMA));
    }

    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    let map = servers
        .as_object_mut()
        .ok_or_else(|| format!("{}: mcpServers is not an object", path.display()))?;

    if map.get(SERVER_NAME) == Some(entry) {
        return Ok(Change::Unchanged);
    }
    map.insert(SERVER_NAME.to_string(), entry.clone());

    if dry_run {
        println!(
            "would write {}",
            serde_json::to_string_pretty(&doc).unwrap()
        );
        return Ok(if existed {
            Change::Updated
        } else {
            Change::Created
        });
    }

    write_atomic(
        path,
        format!("{}\n", serde_json::to_string_pretty(&doc).unwrap()).as_bytes(),
    )?;
    Ok(if existed {
        Change::Updated
    } else {
        Change::Created
    })
}

fn upsert_codex(path: &Path, block: &str, dry_run: bool) -> Result<Change, String> {
    let original = if path.exists() {
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?
    } else {
        String::new()
    };
    let existed = path.exists();
    let header = format!("[mcp_servers.{SERVER_NAME}]");
    let lines: Vec<&str> = original.lines().collect();
    let start = lines.iter().position(|line| line.trim() == header);

    let new_text = if let Some(start) = start {
        let end = lines[start + 1..]
            .iter()
            .position(|line| line.trim_start().starts_with('['))
            .map(|offset| start + 1 + offset)
            .unwrap_or(lines.len());
        let existing = format!("{}\n", lines[start..end].join("\n"));
        if existing == block {
            return Ok(Change::Unchanged);
        }
        let mut out = lines[..start].join("\n");
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block);
        if end < lines.len() {
            out.push('\n');
            out.push_str(&lines[end..].join("\n"));
            out.push('\n');
        }
        out
    } else {
        let mut out = original.clone();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(block);
        out
    };

    if dry_run {
        println!("would write:\n{new_text}");
        return Ok(if existed {
            Change::Updated
        } else {
            Change::Created
        });
    }

    write_atomic(path, new_text.as_bytes())?;
    Ok(if existed {
        Change::Updated
    } else {
        Change::Created
    })
}

fn change_word(change: Change, dry_run: bool) -> &'static str {
    match (change, dry_run) {
        (Change::Unchanged, _) => "unchanged",
        (Change::Created, true) => "would create",
        (Change::Updated, true) => "would update",
        (Change::Created, false) => "created",
        (Change::Updated, false) => "updated",
    }
}

/// Write a file atomically: a per-call tmp file in the target's own
/// directory, permissions preserved from an existing target (0600 for a
/// new one), then a rename. A backup of the previous file is made first.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    if path.exists() {
        let backup = backup_path(path);
        fs::copy(path, &backup).map_err(|e| {
            format!(
                "cannot back up {} to {}: {e}",
                path.display(),
                backup.display()
            )
        })?;
        eprintln!("backup: {}", backup.display());
    }

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let tmp = dir.join(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .map_err(|e| format!("cannot create {}: {e}", tmp.display()))?;
        file.write_all(bytes)
            .and_then(|()| file.flush())
            .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
        drop(file);
        if path.exists() {
            let perms = fs::metadata(path)
                .map_err(|e| format!("cannot stat {}: {e}", path.display()))?
                .permissions();
            fs::set_permissions(&tmp, perms)
                .map_err(|e| format!("cannot set permissions on {}: {e}", tmp.display()))?;
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
                    .map_err(|e| format!("cannot set permissions on {}: {e}", tmp.display()))?;
            }
        }
        fs::rename(&tmp, path)
            .map_err(|e| format!("cannot rename {} into place: {e}", tmp.display()))
    })();
    if outcome.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    outcome
}

fn backup_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "config".to_string());
    let mut n = 0u32;
    loop {
        let suffix = if n == 0 {
            format!("{file_name}.bak-{stamp}")
        } else {
            format!("{file_name}.bak-{stamp}-{n}")
        };
        let candidate = path.with_file_name(suffix);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn parse_args() -> Result<Options, String> {
    let mut opts = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "install" => set_action(&mut opts, Action::Install)?,
            "print" | "--print" => set_action(&mut opts, Action::Print)?,
            "--help" | "-h" => {
                println!("usage: total-recall-mcp install|print [--harness all|omp,pi,claude,codex] [--root PATH] [--binary PATH] [--dry-run] [--force] [--omp-config PATH] [--pi-config PATH] [--claude-config PATH] [--codex-config PATH]");
                std::process::exit(0);
            }
            "--harness" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--harness requires a value".to_string())?;
                for name in value.split(',') {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err("--harness contains an empty name".to_string());
                    }
                    if name == "all" {
                        for h in Harness::all() {
                            if !opts.harnesses.contains(&h) {
                                opts.harnesses.push(h);
                            }
                        }
                    } else {
                        let h = match name {
                            "omp" => Harness::Omp,
                            "pi" => Harness::Pi,
                            "claude" => Harness::Claude,
                            "codex" => Harness::Codex,
                            other => {
                                return Err(format!(
                                    "unknown harness {other:?} (expected omp, pi, claude, codex, or all)"
                                ))
                            }
                        };
                        if !opts.harnesses.contains(&h) {
                            opts.harnesses.push(h);
                        }
                    }
                }
            }
            "--root" => opts.root = Some(next_value(&mut args, "--root")?),
            "--binary" => opts.binary = Some(next_value(&mut args, "--binary")?),
            "--omp-config" => {
                opts.omp_config = Some(PathBuf::from(next_value(&mut args, "--omp-config")?))
            }
            "--pi-config" => {
                opts.pi_config = Some(PathBuf::from(next_value(&mut args, "--pi-config")?))
            }
            "--claude-config" => {
                opts.claude_config = Some(PathBuf::from(next_value(&mut args, "--claude-config")?))
            }
            "--codex-config" => {
                opts.codex_config = Some(PathBuf::from(next_value(&mut args, "--codex-config")?))
            }
            "--dry-run" => opts.dry_run = true,
            "--force" => opts.force = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(opts)
}

fn set_action(opts: &mut Options, action: Action) -> Result<(), String> {
    if let Some(existing) = opts.action {
        if existing != action {
            return Err("specify only one of install or print".to_string());
        }
    }
    opts.action = Some(action);
    Ok(())
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_escape_quotes_and_backslashes() {
        assert_eq!(toml_string(r#"/a\b"c"#), r#""/a\\b\"c""#);
    }

    #[test]
    fn codex_block_is_a_parseable_table_header() {
        let block = codex_block(Path::new("/bin/total-recall"), Path::new("/tmp/memory"));
        assert!(block.starts_with("[mcp_servers.total-recall]\n"));
        assert!(block.contains("command = \"/bin/total-recall\""));
        assert!(block.contains("args = [\"--root\", \"/tmp/memory\"]"));
    }

    #[test]
    fn harness_names_are_the_supported_set() {
        let names: Vec<&str> = Harness::all().iter().map(|h| h.label()).collect();
        assert_eq!(names, ["omp", "pi", "claude", "codex"]);
    }

    #[test]
    fn json_entry_shapes_match_each_harness() {
        let bin = Path::new("/bin/total-recall");
        let root = Path::new("/tmp/memory");
        assert_eq!(omp_entry(bin, root)["type"], "stdio");
        assert_eq!(omp_entry(bin, root)["timeout"], 60_000);
        assert_eq!(pi_entry(bin, root)["exposure"], "direct");
        assert_eq!(pi_entry(bin, root)["timeout"], 60);
        assert_eq!(claude_entry(bin, root)["type"], "stdio");
        assert_eq!(claude_entry(bin, root)["command"], "/bin/total-recall");
    }
}
