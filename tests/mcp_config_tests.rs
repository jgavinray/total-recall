//! Integration tests for `total-recall-mcp` (the harness config helper).
//!
//! Every test passes explicit `--root`, `--binary`, and per-harness config
//! paths, so it neither reads the host's `$HOME` nor writes to real harness
//! config files.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tr-mcp-config-{}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn helper() -> &'static str {
    env!("CARGO_BIN_EXE_total-recall-mcp")
}

fn server() -> String {
    // The install flow records the CANONICALIZED binary path
    // (canonicalize_existing_prefix — symlink-proof by design), so the
    // expectation must be canonical too. On macOS the compile-time cargo
    // path crosses /tmp -> /private/tmp, where the verbatim
    // CARGO_BIN_EXE string differs from the recorded one.
    std::fs::canonicalize(env!("CARGO_BIN_EXE_total-recall"))
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_BIN_EXE_total-recall")))
        .display()
        .to_string()
}

fn run(args: &[String]) -> Output {
    Command::new(helper())
        .args(args)
        .env_remove("EXOMEMORY_DIR")
        .env_remove("EXO_DIR")
        .output()
        .expect("run total-recall-mcp")
}

fn paths(dir: &Path) -> Vec<String> {
    vec![
        dir.join("omp.json").display().to_string(),
        dir.join("pi.json").display().to_string(),
        dir.join("claude.json").display().to_string(),
        dir.join("codex.toml").display().to_string(),
    ]
}

fn base_args(dir: &Path) -> Vec<String> {
    let p = paths(dir);
    vec![
        "--root".to_string(),
        dir.join("memory").display().to_string(),
        "--binary".to_string(),
        server().to_string(),
        "--omp-config".to_string(),
        p[0].clone(),
        "--pi-config".to_string(),
        p[1].clone(),
        "--claude-config".to_string(),
        p[2].clone(),
        "--codex-config".to_string(),
        p[3].clone(),
    ]
}

fn install_args(dir: &Path) -> Vec<String> {
    let mut args = vec!["install".to_string()];
    args.extend(base_args(dir));
    args
}

fn print_args(dir: &Path) -> Vec<String> {
    let mut args = vec!["print".to_string()];
    args.extend(base_args(dir));
    args
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn print_emits_all_four_harness_snippets_without_creating_the_root() {
    let dir = tmp_root("print");
    let out = run(&print_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for path in paths(&dir) {
        assert!(stdout.contains(&path), "missing config path {path}");
    }
    assert!(stdout.contains("\"mcpServers\""));
    assert!(stdout.contains("\"exposure\": \"direct\""));
    assert!(stdout.contains("[mcp_servers.total-recall]"));
    assert!(stdout.contains("mcp__total_recall_read_signoff"));
    assert!(
        !dir.join("memory").exists(),
        "print must not provision the root"
    );
}

#[test]
fn install_writes_every_harness_and_seeds_the_root() {
    let dir = tmp_root("install");
    let out = run(&install_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p = paths(&dir);

    let omp = read_json(Path::new(&p[0]));
    assert_eq!(omp["$schema"].as_str().unwrap(), "https://raw.githubusercontent.com/can1357/oh-my-pi/main/packages/coding-agent/src/config/mcp-schema.json");
    assert_eq!(omp["mcpServers"]["total-recall"]["type"], "stdio");
    assert_eq!(omp["mcpServers"]["total-recall"]["command"], server());
    assert_eq!(omp["mcpServers"]["total-recall"]["timeout"], 60000);
    assert_eq!(omp["mcpServers"]["total-recall"]["args"][0], "--root");

    let pi = read_json(Path::new(&p[1]));
    assert_eq!(pi["mcpServers"]["total-recall"]["exposure"], "direct");
    assert_eq!(pi["mcpServers"]["total-recall"]["timeout"], 60);
    assert_eq!(pi["mcpServers"]["total-recall"]["command"], server());

    let claude = read_json(Path::new(&p[2]));
    assert_eq!(claude["mcpServers"]["total-recall"]["type"], "stdio");
    assert_eq!(claude["mcpServers"]["total-recall"]["command"], server());

    let codex = std::fs::read_to_string(&p[3]).unwrap();
    assert!(codex.contains("[mcp_servers.total-recall]"));
    assert!(codex.contains(&format!("command = \"{}\"", server())));
    assert!(codex.contains("startup_timeout_sec = 10"));
    assert!(codex.contains("tool_timeout_sec = 60"));

    let signoff = std::fs::read_to_string(dir.join("memory/signoff.md")).unwrap();
    assert!(signoff.contains("## If you read nothing else"));
    assert!(signoff.contains("## Worker sessions"));
    assert!(dir.join("memory/.state/init.json").exists());
}

#[test]
fn install_is_idempotent_and_reports_unchanged() {
    let dir = tmp_root("idempotent");
    let first = run(&install_args(&dir));
    assert!(first.status.success());
    let before: Vec<Vec<u8>> = paths(&dir)
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();

    let second = run(&install_args(&dir));
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    for name in ["omp", "pi", "claude", "codex"] {
        assert!(
            stdout.contains(&format!("{name}: unchanged")),
            "second run must report {name} unchanged: {stdout}"
        );
    }
    let after: Vec<Vec<u8>> = paths(&dir)
        .iter()
        .map(|p| std::fs::read(p).unwrap())
        .collect();
    assert_eq!(
        before, after,
        "a second install must not rewrite any config"
    );
}

#[test]
fn install_preserves_unrelated_configuration() {
    let dir = tmp_root("preserve");
    let p = paths(&dir);
    std::fs::write(
        &p[0],
        r#"{"$schema":"custom-schema","mcpServers":{"other":{"command":"x"}}}"#,
    )
    .unwrap();
    std::fs::write(
        &p[3],
        "model = \"gpt\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
    )
    .unwrap();

    let out = run(&install_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let omp = read_json(Path::new(&p[0]));
    assert_eq!(omp["$schema"], "custom-schema");
    assert_eq!(omp["mcpServers"]["other"]["command"], "x");
    assert_eq!(omp["mcpServers"]["total-recall"]["command"], server());

    let codex = std::fs::read_to_string(&p[3]).unwrap();
    assert!(codex.contains("model = \"gpt\""));
    assert!(codex.contains("[mcp_servers.other]"));
    assert!(codex.contains("[mcp_servers.total-recall]"));
}

#[test]
fn install_replaces_an_existing_codex_block_without_duplicating_it() {
    let dir = tmp_root("codex-replace");
    let p = paths(&dir);
    std::fs::write(
        &p[3],
        "[mcp_servers.total-recall]\ncommand = \"old\"\nargs = [\"old\"]\n\n[mcp_servers.other]\ncommand = \"x\"\n",
    )
    .unwrap();

    let out = run(&install_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let codex = std::fs::read_to_string(&p[3]).unwrap();
    assert_eq!(codex.matches("[mcp_servers.total-recall]").count(), 1);
    assert!(!codex.contains("command = \"old\""));
    assert!(codex.contains("[mcp_servers.other]"));
}

#[test]
fn install_selected_harness_only_writes_that_file() {
    let dir = tmp_root("selected");
    let p = paths(&dir);
    let mut args = install_args(&dir);
    args.insert(1, "--harness".to_string());
    args.insert(2, "omp".to_string());
    let out = run(&args);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(Path::new(&p[0]).exists());
    for path in &p[1..] {
        assert!(!Path::new(path).exists(), "{path} must not be written");
    }
}

#[test]
fn install_does_not_overwrite_an_existing_signoff() {
    let dir = tmp_root("seed");
    let root = dir.join("memory");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("signoff.md"), "SENTINEL\n").unwrap();

    let out = run(&install_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("signoff.md")).unwrap(),
        "SENTINEL\n"
    );
}

#[test]
fn dry_run_touches_no_disk() {
    let dir = tmp_root("dry-run");
    let mut args = install_args(&dir);
    args.push("--dry-run".to_string());
    let out = run(&args);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dir.join("memory").exists(),
        "dry run must not create the root"
    );
    for path in paths(&dir) {
        assert!(!Path::new(&path).exists(), "dry run must not write {path}");
    }
}

#[test]
fn root_divergence_from_exomemory_dir_is_refused_unless_forced() {
    let dir = tmp_root("divergence");
    let other = dir.join("other-root");
    std::fs::create_dir_all(&other).unwrap();

    let mut args = install_args(&dir);
    let out = Command::new(helper())
        .args(&args)
        .env("EXOMEMORY_DIR", &other)
        .env_remove("EXO_DIR")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("disagrees with EXOMEMORY_DIR"));
    for path in paths(&dir) {
        assert!(!Path::new(&path).exists(), "refusal must write no config");
    }

    args.push("--force".to_string());
    let forced = Command::new(helper())
        .args(&args)
        .env("EXOMEMORY_DIR", &other)
        .env_remove("EXO_DIR")
        .output()
        .unwrap();
    assert!(
        forced.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(Path::new(&paths(&dir)[0]).exists());
}

#[test]
fn existing_configs_are_backed_up_before_rewrite() {
    let dir = tmp_root("backup");
    let p = paths(&dir);
    std::fs::write(&p[0], r#"{"mcpServers":{}}"#).unwrap();
    let out = run(&install_args(&dir));
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let backups: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("omp.json.bak-"))
        .collect();
    assert_eq!(backups.len(), 1, "one backup expected, got {backups:?}");
}

#[cfg(unix)]
#[test]
fn dry_run_accepts_an_env_root_spelled_through_a_symlink() {
    let dir = tmp_root("env-symlink");
    let real = dir.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let mut args = install_args(&dir);
    args.push("--dry-run".to_string());
    // --root names real/memory, which does not exist yet; EXOMEMORY_DIR
    // names the same directory through the symlink.
    let i = args.iter().position(|a| a == "--root").unwrap();
    args[i + 1] = real.join("memory").display().to_string();
    let out = Command::new(helper())
        .args(&args)
        .env("EXOMEMORY_DIR", link.join("memory"))
        .env_remove("EXO_DIR")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "one directory under two spellings is not a disagreement: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
