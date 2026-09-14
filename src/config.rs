//! Memory-root configuration and the init fingerprint (spec §2).
//!
//! Precedence: `--root` flag > `EXOMEMORY_DIR` > `EXO_DIR` (legacy alias)
//! > `~/.config/exomemory/config.toml` key `root` > default
//! `~/dev/exomemory/`. The default is a convenience for the reference
//! deployment, NOT part of the contract — any directory satisfying the
//! spec's storage layout is a valid root.
//!
//! Loud refusals (spec §2, §13):
//! - both env names set to different paths → `Err`, even when a flag
//!   overrides (two planes on two directories is the misconfiguration
//!   the spec refuses);
//! - an existing init fingerprint for the same root recording a
//!   DIFFERENT timezone/offset → `Err` (a mixed-TZ multi-box shared
//!   root is a misconfiguration, not a supported mode);
//! - an `EXO_TZ` override the system tz database does not know → `Err`
//!   (an unrecognized zone makes `date` fall back to UTC silently on
//!   both macOS and glibc, which would mislabel day boundaries instead
//!   of refusing).

use std::path::{Path, PathBuf};

use serde_json::json;

/// CLI surface. Pinned by the build contract: one field, constructed by
/// literal — the gate builds `ConfigArgs { cli_root: … }`.
#[derive(Debug, Clone, Default)]
pub struct ConfigArgs {
    /// Value of `--root <path>`; `None` when the flag is not given.
    pub cli_root: Option<String>,
}

/// The default memory root, relative to `$HOME`. A convenience for the
/// reference deployment, not part of the contract (spec §2).
pub const DEFAULT_ROOT: &str = "dev/exomemory/";

/// Config file consulted when no flag/env answers the root (spec §2
/// Configuration, tier 3), relative to `$HOME`.
pub const CONFIG_FILE_RELATIVE: &str = ".config/exomemory/config.toml";

/// Zone databases probed to verify an `EXO_TZ` name before trusting
/// `date` with it (macOS ships `/var/db/timezone/zoneinfo`, most Linux
/// `/usr/share/zoneinfo` or `/usr/lib/zoneinfo`).
const ZONEINFO_BASES: &[&str] =
    &["/var/db/timezone/zoneinfo", "/usr/share/zoneinfo", "/usr/lib/zoneinfo"];

/// Resolves the memory root (spec §2 precedence: flag > `EXOMEMORY_DIR`
/// > `EXO_DIR` > config file > default).
///
/// Loud refusal: if BOTH env names are set to different paths this is an
/// `Err` — even when the flag overrides. The guard plane reads
/// `EXOMEMORY_DIR`; the two env names disagreeing means two planes on two
/// directories, which the spec refuses at startup (spec §2, §13).
pub fn resolve_root(args: &ConfigArgs) -> Result<PathBuf, String> {
    let canonical = std::env::var("EXOMEMORY_DIR").ok().filter(|s| !s.trim().is_empty());
    let legacy = std::env::var("EXO_DIR").ok().filter(|s| !s.trim().is_empty());

    if let (Some(a), Some(b)) = (&canonical, &legacy) {
        if a != b {
            return Err("EXOMEMORY_DIR and EXO_DIR resolve to different roots — misconfiguration".into());
        }
    }

    let chosen = if let Some(flag) = &args.cli_root {
        if flag.trim().is_empty() {
            return Err("--root is empty — refusing to start (point it at the memory root directory)".into());
        }
        Some(flag.clone())
    } else if let Some(env) = &canonical {
        Some(env.clone())
    } else if let Some(env) = &legacy {
        Some(env.clone())
    } else {
        // Tier 4: config file. Its location is `$HOME`-relative; without
        // a home directory the file is simply not consulted.
        home_dir().and_then(|home| config_file_root(&home.join(CONFIG_FILE_RELATIVE)))
    };

    match chosen {
        Some(raw) => expand_tilde(&raw),
        None => home_dir()
            .map(|home| home.join(DEFAULT_ROOT))
            .ok_or_else(|| {
                "no memory root resolvable: --root, EXOMEMORY_DIR and EXO_DIR are unset, no ~/.config/exomemory/config.toml key root, and $HOME is unset — cannot fall back to the default ~/dev/exomemory/".to_string()
            }),
    }
}

/// Reads the `root` key from a TOML config file (spec §2 tier 3).
///
/// The server's config is one key; this is a deliberately minimal TOML
/// reader (std-only, per §2's dependency rule): top-level `key = value`
/// lines, `#` comments, basic `"…"` and literal `'…'` strings, and bare
/// values terminated by a comment. Returns `None` when the file is
/// absent, unreadable, or carries no usable `root` key. `~` expansion
/// happens at resolution (`resolve_root`), so the raw value is returned.
pub fn config_file_root(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue, // malformed line: ignore, keep scanning
        };
        if key.trim() != "root" {
            continue;
        }
        return parse_toml_value(rest);
    }
    None
}

/// Right-hand side of a minimal-TOML `key = value`: a quoted basic or
/// literal string, or a bare value terminated by a `#` comment.
fn parse_toml_value(rest: &str) -> Option<String> {
    let rest = rest.trim();
    let body = if let Some(inner) = rest.strip_prefix('"') {
        // Basic string: comment after the closing quote is not content.
        let end = inner.find('"')?;
        inner.get(..end)?.to_string()
    } else if let Some(inner) = rest.strip_prefix('\'') {
        let end = inner.find('\'')?;
        inner.get(..end)?.to_string()
    } else {
        match rest.find('#') {
            Some(i) => &rest[..i],
            None => rest,
        }
        .trim()
        .to_string()
    };
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

/// Expands a leading `~` / `~/` against `$HOME`; everything else is
/// taken as-is (relative paths stay relative to the caller's CWD).
fn expand_tilde(raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw == "~" {
        home_dir().ok_or_else(|| "the root uses '~' but $HOME is unset — refusing to start".into())
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home_dir()
            .map(|home| home.join(rest))
            .ok_or_else(|| "the root uses '~' but $HOME is unset — refusing to start".to_string())
    } else {
        Ok(PathBuf::from(raw))
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

/// The zone + UTC offset (minutes) in effect right now: the `EXO_TZ`
/// IANA override if set, else the system local zone. A single configured
/// memory root is single-timezone by construction (spec §2) — this is
/// what the init fingerprint keys the root to at first init.
pub fn resolve_tz() -> Result<(String, i64), String> {
    match std::env::var("EXO_TZ") {
        Ok(zone) if !zone.trim().is_empty() => {
            let zone = zone.trim().to_string();
            zone_offset_minutes(&zone).map(|offset| (zone, offset))
        }
        _ => {
            let zone = system_zone_name();
            system_offset_minutes().map(|offset| (zone, offset))
        }
    }
}

/// Best-effort IANA name of the system local zone: the target of
/// `/etc/localtime` (macOS `/var/db/timezone/zoneinfo/…`, most Linux
/// `/usr/share/zoneinfo/…`), else the Debian-style `/etc/timezone` file,
/// else the marker `"local"` (the offset still resolves from the clock).
fn system_zone_name() -> String {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let lossy = target.to_string_lossy();
        for base in ZONEINFO_BASES {
            if let Some(at) = lossy.find(base) {
                let name = lossy[at + base.len()..].trim_start_matches('/');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string("/etc/timezone") {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "local".to_string()
}

/// UTC offset (signed minutes) of an explicitly named zone, run through
/// the system tz database (`TZ=<zone> date +%z`).
///
/// Loud refusal up front: an unknown zone makes `date` fall back to UTC
/// silently on macOS and glibc alike, so the zone is verified against
/// the zoneinfo database first — a wrong zone must refuse, never
/// mislabel day boundaries.
fn zone_offset_minutes(zone: &str) -> Result<i64, String> {
    if zone.is_empty() || zone.contains("..") || Path::new(zone).is_absolute() {
        return Err(format!(
            "EXO_TZ='{zone}' is not a usable IANA zone (expected a relative zone path like 'America/New_York') — refusing to start"
        ));
    }
    if !ZONEINFO_BASES
        .iter()
        .any(|base| Path::new(base).join(zone).is_file()
            || Path::new(base).join(format!("{zone}0")).is_file())
    {
        return Err(format!(
            "EXO_TZ='{zone}' is not a known IANA timezone on this system — refusing to start (an unrecognized zone would fall back to UTC silently and mislabel every day boundary)"
        ));
    }
    let output = std::process::Command::new("date")
        .env("TZ", zone)
        .arg("+%z")
        .output()
        .map_err(|e| format!("cannot run `date` to resolve the timezone offset for '{zone}': {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "the system timezone database rejects '{zone}' — refusing to start"
        ));
    }
    parse_utc_offset(&String::from_utf8_lossy(&output.stdout))
        .map_err(|e| format!("timezone '{zone}' produced an unparsable UTC offset: {e}"))
}

/// UTC offset (signed minutes) of the system local zone (bare `date`,
/// no `TZ` override — the zone the box's clock actually reports).
fn system_offset_minutes() -> Result<i64, String> {
    let output = std::process::Command::new("date")
        .arg("+%z")
        .output()
        .map_err(|e| format!("cannot run `date` to resolve the system UTC offset: {e}"))?;
    if !output.status.success() {
        return Err("the system clock reports no usable UTC offset — refusing to start".into());
    }
    parse_utc_offset(&String::from_utf8_lossy(&output.stdout))
        .map_err(|e| format!("the system UTC offset is unparsable: {e}"))
}

/// Parses a `+HHMM` / `-HHMM` UTC offset into signed minutes.
fn parse_utc_offset(raw: &str) -> Result<i64, String> {
    let raw = raw.trim();
    if raw.len() != 5 {
        return Err(format!("expected a +HHMM offset, got {raw:?}"));
    }
    let sign: i64 = match raw.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err(format!("expected a leading '+' or '-', got {raw:?}")),
    };
    let digits = &raw[1..];
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("non-digits in offset {raw:?}"));
    }
    let hours: i64 = digits
        .get(0..2)
        .and_then(|h| h.parse().ok())
        .ok_or_else(|| format!("bad hours in offset {raw:?}"))?;
    let minutes: i64 = digits
        .get(2..4)
        .and_then(|m| m.parse().ok())
        .ok_or_else(|| format!("bad minutes in offset {raw:?}"))?;
    if hours > 14 || minutes >= 60 {
        return Err(format!("implausible UTC offset {raw:?}"));
    }
    Ok(sign * (hours * 60 + minutes))
}

/// Writes the init fingerprint for `root` (spec §2, `.state/` row).
///
/// - fresh root: creates `.state/` and writes `init.json` — nothing
///   else. A fresh configured root starts EMPTY; every other bucket is
///   created-on-first-write;
/// - same root, same zone/offset: idempotent, no rewrite;
/// - same root with a DIFFERENT zone/offset: refuses loudly — a mixed-TZ
///   multi-box shared root is a misconfiguration, not a supported mode
///   (spec §2 timezone/day boundary);
/// - a fingerprint that names a DIFFERENT root: refuses loudly (the
///   directory was moved or copied — the keying would silently lie).
///
/// `init.json` = `{root, tz, tz_offset_minutes, created_at}`: `root` is
/// the resolved absolute path, `tz`/`tz_offset_minutes` per
/// `resolve_tz`, `created_at` UTC ISO-8601 with a trailing `Z`.
pub fn init_state(root: &Path) -> Result<(), String> {
    let (tz, offset) = resolve_tz()?;
    let abs = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(root))
            .map_err(|e| format!("cannot resolve relative root {:?}: {e}", root.display()))?
    };

    std::fs::create_dir_all(&abs)
        .map_err(|e| format!("cannot create memory root {}: {e}", abs.display()))?;
    let state_dir = abs.join(".state");
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("cannot create {}.state: {e}", abs.display()))?;
    let state_path = state_dir.join("init.json");

    if state_path.exists() {
        let old = match read_fingerprint(&state_path) {
            Some(fp) => fp,
            None => {
                return Err(format!(
                    "{} exists but is not a readable init fingerprint — refusing to start (delete it to re-init)",
                    state_path.display()
                ))
            }
        };
        if old.root != abs.display().to_string() {
            return Err(format!(
                "refusing to start: {} names memory root '{}' but this directory resolves to '{}' — the fingerprint is keyed to its directory; if the root was moved or copied, delete .state/init.json and re-init",
                state_path.display(),
                old.root,
                abs.display()
            ));
        }
        if old.tz != tz || old.tz_offset_minutes != offset {
            return Err(format!(
                "refusing to start: timezone change for memory root '{}' — {} records timezone '{}' (UTC offset {} min, created {}) but this box now resolves '{}' (UTC offset {} min). A mixed-TZ multi-box shared root is a misconfiguration, not a supported mode; if the box legitimately changed zone, delete .state/init.json and re-init",
                abs.display(),
                state_path.display(),
                old.tz,
                old.tz_offset_minutes,
                old.created_at,
                tz,
                offset
            ));
        }
        return Ok(());
    }

    let doc = json!({
        "root": abs.display().to_string(),
        "tz": tz,
        "tz_offset_minutes": offset,
        "created_at": now_utc_iso8601(),
    });
    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| format!("cannot serialize the init fingerprint: {e}"))?;
    std::fs::write(&state_path, format!("{text}\n"))
        .map_err(|e| format!("cannot write {}: {e}", state_path.display()))?;
    Ok(())
}

/// The on-disk fingerprint. Parsed by hand (no serde derive — the
/// dependency set is std + serde_json only, spec §2).
#[derive(Debug, Clone)]
struct Fingerprint {
    root: String,
    tz: String,
    tz_offset_minutes: i64,
    created_at: String,
}

fn read_fingerprint(path: &Path) -> Option<Fingerprint> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(Fingerprint {
        root: v.get("root").and_then(|x| x.as_str())?.to_string(),
        tz: v.get("tz").and_then(|x| x.as_str())?.to_string(),
        tz_offset_minutes: v.get("tz_offset_minutes").and_then(|x| x.as_i64())?,
        created_at: v.get("created_at").and_then(|x| x.as_str())?.to_string(),
    })
}

/// Current UTC wall-clock as `YYYY-MM-DDTHH:MM:SSZ` — all emitted
/// timestamps are UTC with a trailing `Z` (spec §2).
fn now_utc_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3_600, rem / 60 % 60, rem % 60);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        month, day, hour, minute, second
    )
}

/// Howard Hinnant's `civil_from_days`: pure-integer calendar arithmetic
/// for the UTC date (no third-party date dependency, per §2). Days are
/// counted from the Unix epoch; internally they are re-based to the
/// March-anchored proleptic Gregorian calendar (719468 days between
/// 0000-03-01 and 1970-01-01) and divided into 400-year eras
/// (146097 days — the calendar's exact repetition period), so the
/// century leap rules fall out of the integer math.
fn civil_from_days(d: i64) -> (i64, u32, u32) {
    let z = d + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097; // floor
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let y = era * 400 + yoe;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let (y, month) = if mp < 10 {
        (y, mp + 3) // March..December stay in the same year
    } else {
        (y + 1, mp - 9) // January/February belong to the next year
    };
    (y, month as u32, day as u32)
}

#[cfg(test)]
mod calendar_tests {
    use super::civil_from_days;

    /// Calendar arithmetic pinned against `date -u` (the system's own
    /// calendar) at known epoch-day values: the epoch, year boundaries,
    /// leap days, and the century leap rule (1900 and 2100 are NOT
    /// leap years, 2000 IS).
    #[test]
    fn civil_from_days_matches_oracle_dates() {
        let vectors: &[(i64, i64, u32, u32)] = &[
            (-25_508, 1900, 3, 1), // 1900: century, not a leap year
            (-1, 1969, 12, 31), // the day before the epoch
            (0, 1970, 1, 1), // the epoch itself
            (1, 1970, 1, 2),
            (10_957, 2000, 1, 1), // 2000: the century that IS a leap year
            (11_021, 2000, 3, 5),
            (11_022, 2000, 3, 6),
            (11_081, 2000, 5, 4),
            (11_082, 2000, 5, 5),
            (19_722, 2023, 12, 31), // year boundary
            (19_723, 2024, 1, 1),
            (19_781, 2024, 2, 28),
            (19_782, 2024, 2, 29), // leap day
            (19_783, 2024, 3, 1),
            (20_454, 2026, 1, 1),
            (20_709, 2026, 9, 13),
            (47_540, 2100, 2, 28), // 2100: century, not a leap year
        ];
        for &(d, y, m, day) in vectors {
            let (gy, gm, gday) = civil_from_days(d);
            assert_eq!(
                (gy, gm, gday),
                (y, m, day),
                "civil_from_days({d}) mismatched the oracle"
            );
        }
    }
}
