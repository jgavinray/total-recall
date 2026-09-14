//! Worker self-checks for `exomem_mcp::config` (ExoM0, wave 1).
//!
//! Module-scoped only, and deliberately NON-OVERLAPPING with the
//! orchestrator gate (`tests/gate_w1.rs`, which must not be edited):
//!
//! - the gate pins flag > env, env divergence (non-empty Err), the
//!   fingerprint's field presence, and per-bucket absence on a fresh
//!   root. This file covers the rest of the matrix:
//! - the legacy `EXO_DIR` tier answering on its own;
//! - equal `EXOMEMORY_DIR`/`EXO_DIR` values NOT refusing, and the
//!   divergence message naming both variables even under a flag;
//! - the config-file tier (`~/.config/exomemory/config.toml` key
//!   `root`) resolving + expanding `~`, tested through a `$HOME` the
//!   test points at a scratch dir (the real one is never touched);
//! - the `EXO_TZ` IANA override being honored in the fingerprint, a
//!   zone change for the same root refusing loudly (and rewriting
//!   nothing), a matching re-init staying idempotent, and an unknown
//!   zone refusing loudly instead of falling back to UTC;
//! - the fresh-root INVENTORY (exactly `.state/`, containing exactly
//!   `init.json`) — the gate asserts per-file absence, this asserts
//!   the full listing;
//! - `created_at` as real UTC ISO-8601 with a trailing `Z` (the gate
//!   asserts presence, this asserts the shape);
//! - a fingerprint naming a different root refusing loudly (the gate
//!   forges `tz`; this forges `root`).

use exomem_mcp::config::{self, ConfigArgs};
use std::path::PathBuf;
use std::sync::Mutex;

// Test threads share one process: every test that reads or writes the
// root/env environment (or `$HOME`) serializes on this.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scratch root under the system temp dir, re-created fresh (named
/// per test so parallel tests never share a directory).
fn tmp_root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "exomem-cfg-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Unset the three root/env variables the module reads.
fn clear_root_env() {
    std::env::remove_var("EXOMEMORY_DIR");
    std::env::remove_var("EXO_DIR");
    std::env::remove_var("EXO_TZ");
}

#[test]
fn exo_dir_legacy_alias_answers_when_canonical_unset() {
    let _g = env_guard();
    let dir = tmp_root("exodir");
    clear_root_env();
    std::env::set_var("EXO_DIR", dir.display().to_string());
    let cfg = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap();
    assert_eq!(cfg, dir, "the legacy EXO_DIR tier must answer the root on its own");
    clear_root_env();
}

#[test]
fn both_env_names_equal_is_not_refused() {
    let _g = env_guard();
    let dir = tmp_root("envsequal");
    clear_root_env();
    std::env::set_var("EXOMEMORY_DIR", dir.display().to_string());
    std::env::set_var("EXO_DIR", dir.display().to_string());
    let cfg = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap();
    assert_eq!(
        cfg, dir,
        "identical values in both names are one plane, not a divergence"
    );
    clear_root_env();
}

#[test]
fn env_divergence_names_both_variables_and_flag_cannot_paper_over_it() {
    let _g = env_guard();
    let a = tmp_root("div-a");
    let b = tmp_root("div-b");
    clear_root_env();
    std::env::set_var("EXOMEMORY_DIR", a.display().to_string());
    std::env::set_var("EXO_DIR", b.display().to_string());

    let err = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap_err();
    assert!(
        err.contains("EXOMEMORY_DIR"),
        "the refusal must name the canonical variable: {err}"
    );
    assert!(err.contains("EXO_DIR"), "the refusal must name the legacy alias: {err}");

    // Contract: the divergence is refused even when a flag overrides.
    let err = config::resolve_root(&ConfigArgs {
        cli_root: Some(a.display().to_string()),
    })
    .unwrap_err();
    assert!(!err.is_empty(), "a --root flag must not paper over an env divergence: {err}");

    clear_root_env();
}

#[test]
fn config_file_tier_answers_and_expands_tilde() {
    let _g = env_guard();
    let dir = tmp_root("cfgtier");
    let home = dir.join("home");
    let cfg_path = home.join(".config/exomemory/config.toml");
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();

    // Point $HOME at the scratch home so the module's real config-file
    // location (~/.config/exomemory/config.toml) lands in the scratch
    // tree — the user's real config is never read or written.
    let old_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.display().to_string());
    clear_root_env();

    let target = dir.join("memory-root");
    let toml = format!("# exomemory\nother_key = \"noise\"\nroot = \"{}\"\n", target.display());
    std::fs::write(&cfg_path, toml).unwrap();
    let cfg = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap();
    assert_eq!(cfg, target, "the config-file tier must answer when no flag/env is set");

    // A `~` in the file value expands against $HOME at resolution time.
    std::fs::write(&cfg_path, "root = \"~/tilde-root\"\n").unwrap();
    let cfg = config::resolve_root(&ConfigArgs { cli_root: None }).unwrap();
    assert_eq!(cfg, home.join("tilde-root"), "a ~ in config root must expand");

    match old_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    clear_root_env();
}

#[test]
fn exo_tz_override_honored_mismatch_refused_unknown_zone_loud() {
    let _g = env_guard();
    let dir = tmp_root("tz");
    clear_root_env();

    // 1. The EXO_TZ IANA override is what the fingerprint records.
    std::env::set_var("EXO_TZ", "UTC");
    config::init_state(&dir).unwrap();
    let st = dir.join(".state/init.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&st).unwrap()).unwrap();
    assert_eq!(v["tz"], "UTC", "EXO_TZ must override the system zone in the fingerprint");
    assert_eq!(v["tz_offset_minutes"], 0, "UTC must record offset 0");

    // 2. A later start resolving a DIFFERENT zone for the same root
    //    refuses loudly — and rewrites nothing.
    std::env::set_var("EXO_TZ", "America/New_York");
    let err = config::init_state(&dir).unwrap_err();
    assert!(
        err.contains("timezone") || err.contains("zone"),
        "a zone change for the same root must be loud: {err}"
    );
    let v2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&st).unwrap()).unwrap();
    assert_eq!(v2["tz"], "UTC", "a refused re-init must not rewrite the fingerprint");

    // 3. The same zone/offset again is idempotent (no false refusal).
    std::env::set_var("EXO_TZ", "UTC");
    config::init_state(&dir).unwrap();

    // 4. A zone the tz database does not know is refused loudly — never
    //    a silent fallback to UTC.
    std::env::set_var("EXO_TZ", "Not/AZone");
    let err = config::init_state(&dir).unwrap_err();
    assert!(
        err.contains("EXO_TZ"),
        "an unknown zone must be named in the refusal: {err}"
    );
    clear_root_env();
}

#[test]
fn fresh_root_inventory_is_exactly_state_with_init_json() {
    let _g = env_guard();
    let dir = tmp_root("inventory");
    config::init_state(&dir).unwrap();
    let entries: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        entries,
        vec![".state".to_string()],
        "a fresh configured root starts EMPTY: nothing but .state/ may exist after init"
    );
    let state_entries: Vec<String> = std::fs::read_dir(dir.join(".state"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        state_entries,
        vec!["init.json".to_string()],
        "and .state/ holds exactly the init fingerprint"
    );
    clear_root_env();
}

#[test]
fn created_at_is_utc_iso8601_with_trailing_z() {
    let _g = env_guard();
    let dir = tmp_root("createdat");
    config::init_state(&dir).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".state/init.json")).unwrap())
            .unwrap();
    let ts = v["created_at"].as_str().unwrap().to_string();
    // YYYY-MM-DDTHH:MM:SSZ, exactly 20 chars, digits everywhere else.
    assert_eq!(ts.len(), 20, "ISO-8601 second precision: {ts}");
    for (i, c) in ts.as_bytes().iter().enumerate() {
        let expected = match i {
            4 | 7 => b'-',
            10 => b'T',
            13 | 16 => b':',
            19 => b'Z',
            _ => b'0',
        };
        if i == 4 || i == 7 || i == 10 || i == 13 || i == 16 || i == 19 {
            assert_eq!(*c, expected, "position {i} of {ts}");
        } else {
            assert!(c.is_ascii_digit(), "position {i} of {ts} must be a digit");
        }
    }
    clear_root_env();
}

#[test]
fn fingerprint_named_for_another_root_is_refused() {
    let _g = env_guard();
    let dir = tmp_root("moved");
    config::init_state(&dir).unwrap();
    let st = dir.join(".state/init.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&st).unwrap()).unwrap();
    v["root"] = serde_json::json!("/elsewhere/entirely-different");
    std::fs::write(&st, v.to_string()).unwrap();
    let err = config::init_state(&dir).unwrap_err();
    assert!(
        err.contains("root"),
        "a fingerprint naming a different root must refuse loudly: {err}"
    );
    clear_root_env();
}
