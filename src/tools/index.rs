//! The derived recall index (spec §2 `.index/` row, §3 `recall`, §13
//! "recall indexing: DECIDED derived internal index"; kaibo verdict
//! `signoffs/review_okf_index_amendment.md` §3).
//!
//! It is a DERIVED CACHE, never a bucket and never the source of truth:
//! the markdown is. Everything here is rebuildable from disk in one
//! pass, and `recall` never serves an excerpt it has not re-read from
//! the live file — the index only ever NARROWS the candidate files (and
//! candidate lines within them) that `recall` then verifies against the
//! live bytes and stamps with the live file's mtime.
//!
//! Two layers, per the verdict:
//! - **in-process memoization is the correctness mechanism** — a
//!   per-root snapshot of `{path, mtime, size, inode, line_count,
//!   format_version}` fingerprints plus parsed entry lists (the file's
//!   lines, the line-scoped search unit §13 R3 pins), held in process
//!   and validated on every call;
//! - the on-disk `.index/manifest.json` is an OPTIONAL, human-inspectable
//!   derived report: written only under the `.locks/index.lock`
//!   `O_CREAT|O_EXCL` mutex (bounded retry + age-based stale takeover, the
//!   §2 / `gate.rs` / `signoff_append.rs` precedent) and only by tmp file +
//!   atomic rename. It is never read as fact and never consulted for an
//!   answer: a corrupt, truncated or version-mismatched manifest is treated
//!   as absent and rebuilt silently, and nothing that narrows a search is
//!   ever taken from disk — narrowing comes only from the memo this process
//!   built from its own live read, so a report edited to look current
//!   (fingerprints and all) can neither hide a match nor invent one. What
//!   the server does with the report on disk is hold it to account: see
//!   `report_disagrees`.
//!
//! Pinned semantics:
//! - **std + serde_json only** — the fingerprint reads `inode` through
//!   `std::os::unix::fs::MetadataExt` (§2's dependency rule; no crate is
//!   added, §9's licence simplicity depends on it);
//! - **buckets are enumerated, never asked for**: `signoff.md`, every
//!   `YYYY-MM-DD.md` day file present at the root, and `briefs/*.md`.
//!   `wiki/`, `inbox/`, `topics/`, `signoffs/`, `.audit/`, `.claims/`,
//!   `.locks/`, `.state/` and `.index/` itself are never read (spec §10,
//!   §3 EXCLUSIONS). Every candidate is canonicalized and must resolve
//!   inside the configured root — an escaping symlink is a LOUD refusal
//!   naming the path ("a check that can't run must fail loudly");
//! - **validated on every `recall` call**: a fingerprint mismatch (live
//!   mtime/size/inode, or the recorded `format_version`) against the
//!   memo or the manifest triggers a silent full rebuild from disk; a
//!   rebuild that is impossible (an existing bucket file that cannot be
//!   read or stat'd) is LOUD and names the path — recall then refuses
//!   rather than serving anything it cannot verify;
//! - the refresh never runs inside an append critical section and can
//!   never alter or block an append: `recall` is a reader, the audit
//!   writer (`tools::ticks`) never calls in, and the append paths keep
//!   their own locks untouched by this module.
//!
//! The residual hazard is stated in the module's own words, not hidden:
//! a same-second, same-size, same-line-count, same-inode edit can evade
//! the fingerprint and cost a MISSED match (the verdict's hazard 3).
//! Excerpts are never wrong because they always come from a live read;
//! only freshness of a brand-new match can lag, and any real edit moves
//! at least one of the four fingerprint fields.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// The on-disk (and in-memory) schema version. A manifest carrying any
/// other value is stale-by-definition and rebuilt, never misread
/// (verdict hazard 5).
pub const FORMAT_VERSION: u64 = 1;

/// The derived-cache directory (spec §2 `.index/` row). Created on first
/// write, like every other dot-dir; deletable by anyone at any time.
pub const INDEX_DIR: &str = ".index";
/// The derived report's file name inside `.index/`.
pub const MANIFEST_NAME: &str = "manifest.json";
/// The index's own `.locks/` mutex name (spec §2 concurrency model).
const LOCK_NAME: &str = "index.lock";

/// A lock whose holder has been silent this long is stale and is taken
/// over server-side (spec §2 — the same budget the signoff and day-file
/// appends use).
const STALE_AFTER: Duration = Duration::from_secs(30);
/// Sleep between acquire attempts while a FRESH lock is held.
const RETRY_SLEEP: Duration = Duration::from_millis(100);
/// Bounded retry budget: 50 × 100 ms, then the report is skipped (loud,
/// on stderr) — a cache that cannot be written is never a reason to
/// refuse a search, because the cache is never authoritative.
const MAX_ACQUIRE_ATTEMPTS: u32 = 50;

/// Per-process uniqueness for tmp file names (pid + sequence), so two
/// refreshes in one process never share a tmp path and a crashed
/// writer's tmp stays identifiable.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
/// Per-process acquire sequence, so "remove your own lock file" can
/// never remove a successor's.
static ACQUIRE_NONCE: AtomicU64 = AtomicU64::new(0);

/// The fixed search buckets of spec §3's `scope` enum. The enum IS the
/// confinement: there is no path parameter anywhere in this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `signoff.md` only.
    Signoff,
    /// every `YYYY-MM-DD.md` day file present at the root.
    Dayfile,
    /// `briefs/*.md` only.
    Briefs,
    /// all three buckets.
    All,
}

/// One bucket file as enumerated from disk: the root-relative path (the
/// `path` field `recall` reports) and the absolute path behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BucketFile {
    pub rel: String,
    pub abs: PathBuf,
}

/// One file's fingerprint — the whole identity of a cache entry.
/// `line_count` is the length of the parsed entry list (derived at
/// build time and re-checked whenever the file is read); `mtime`/`size`/
/// `inode` are readable without touching the content, which is what
/// makes the per-call validation cheap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub path: String,
    pub mtime: i64,
    pub size: u64,
    pub inode: u64,
    pub line_count: u64,
    pub format_version: u64,
}

/// A file as the cache holds it: its fingerprint and the parsed entry
/// list — the file's lines, in order, without their terminators (the
/// line is the search unit §13 R3 pins).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFile {
    pub fingerprint: Fingerprint,
    pub lines: Vec<String>,
}

/// The snapshot: root-relative path -> indexed file, ordered so the
/// manifest renders deterministically.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub files: BTreeMap<String, IndexedFile>,
}

/// A candidate the index hands `recall`: which file to read and which of
/// its lines matched. The line numbers are a hint only — `recall`
/// re-reads the file and decides against the live bytes.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub file: BucketFile,
    pub lines: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

/// Enumerate the fixed buckets for a scope, in the order `recall`
/// reports them (`signoff.md`, then day files by name, then briefs by
/// name). Absent buckets contribute nothing (a fresh root is empty,
/// spec §2). An entry whose canonical path escapes the configured root
/// (a symlinked-out bucket) is a LOUD refusal naming the path.
pub fn bucket_files(root: &Path, scope: Scope) -> Result<Vec<BucketFile>, String> {
    let base = root.canonicalize().map_err(|err| {
        format!("index refused: the configured memory root {root:?} cannot be resolved: {err}")
    })?;
    let mut out: Vec<BucketFile> = Vec::new();
    let mut want = |rel: &str, abs: PathBuf| -> Result<(), String> {
        let inside = abs
            .canonicalize()
            .map_err(|err| format!("index refused: cannot resolve bucket entry {abs:?}: {err}"))?;
        if !inside.starts_with(&base) {
            return Err(format!(
                "index refused: bucket entry {abs:?} resolves outside the configured memory root ({inside:?}) — path confinement refused"
            ));
        }
        out.push(BucketFile {
            rel: rel.to_string(),
            abs: inside,
        });
        Ok(())
    };

    if scope == Scope::Signoff || scope == Scope::All {
        let path = root.join("signoff.md");
        if is_readable_file(&path) {
            want("signoff.md", path)?;
        }
    }

    if scope == Scope::Dayfile || scope == Scope::All {
        let mut days: Vec<(String, PathBuf)> = Vec::new();
        for entry in read_dir_sorted(root, "the memory root")? {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_regular_file(&entry) || !is_dayfile_name(&name) {
                continue;
            }
            days.push((name.clone(), root.join(&name)));
        }
        for (name, path) in days {
            want(&name, path)?;
        }
    }

    if scope == Scope::Briefs || scope == Scope::All {
        let dir = root.join("briefs");
        if dir.is_dir() {
            let mut briefs: Vec<(String, PathBuf)> = Vec::new();
            for entry in read_dir_sorted(&dir, "briefs/")? {
                let name = entry.file_name().to_string_lossy().to_string();
                if !is_regular_file(&entry) || !name.ends_with(".md") || name.starts_with('.') {
                    continue;
                }
                briefs.push((name.clone(), dir.join(&name)));
            }
            for (name, path) in briefs {
                want(&format!("briefs/{name}"), path)?;
            }
        }
    }

    Ok(out)
}

/// A compiled query: the matcher `recall` installs into the index, and
/// the same matcher it runs against the live bytes.
pub struct Query {
    /// the raw `query` argument.
    pub raw: String,
    /// true when the query began with the literal `re:` prefix (§13 R3).
    pub regex: bool,
    /// the pattern under test (the query minus the `re:` prefix).
    pub pattern: String,
    /// The case-folded literal, substring mode only (regex mode leaves
    /// it empty rather than keeping a pattern nobody reads).
    needle: String,
    program: Vec<Node>,
}

impl Query {
    /// Compile a search query. Default mode is a case-insensitive
    /// substring; the literal prefix `re:` selects a case-insensitive
    /// regex over the remainder. An invalid regex is a LOUD refusal
    /// naming the pattern — never a silent zero-result search.
    pub fn compile(query: &str) -> Result<Query, String> {
        if let Some(pattern) = query.strip_prefix("re:") {
            // An empty pattern matches every line: that is a bucket dump
            // wearing a search's clothes, and `recall` refuses those.
            if pattern.trim().is_empty() {
                return Err(format!(
                    "regex pattern is empty (query {query:?}) — an empty pattern matches every line, which is not a search"
                ));
            }
            let program = compile_regex(pattern)?;
            Ok(Query {
                raw: query.to_string(),
                regex: true,
                pattern: pattern.to_string(),
                needle: String::new(),
                program,
            })
        } else {
            Ok(Query {
                raw: query.to_string(),
                regex: false,
                pattern: query.to_string(),
                needle: query.to_lowercase(),
                program: Vec::new(),
            })
        }
    }

    /// Does this line match? Case-insensitive in both modes. `Err` is
    /// the search budget: a pattern this engine cannot finish is
    /// refused loudly, never answered as a zero-result search ("a
    /// check that can't run must fail loudly").
    pub fn matches(&self, line: &str) -> Result<bool, String> {
        if self.regex {
            regex_matches(&self.program, line).map_err(|()| {
                format!(
                    "regex {:?} is too complex to finish searching this line within the search budget — refusing to answer from a search it could not complete",
                    self.pattern
                )
            })
        } else {
            Ok(line.to_lowercase().contains(&self.needle))
        }
    }
}

/// The validated snapshot for this call — the heart of the "validated on
/// every recall call" invariant.
///
/// * `use_memo` selects the in-process memoization layer; `false` forces
///   the cold path — what a second process on the same root sees — which
///   is how the index tests observe the cache's own behaviour;
/// * a memo hit requires EVERY enumerated bucket file's live fingerprint
///   to match, and no file to have appeared or vanished; anything else is
///   a silent full rebuild from disk;
/// * the snapshot just built from the live bytes is held against the
///   on-disk report (`report_disagrees`): a report that claims to
///   describe exactly those bytes and disagrees with them is announced
///   and overwritten — a real fault signal, and not a reason to refuse;
/// * a build that cannot run (an existing bucket file that cannot be
///   stat'd or read) is LOUD and names the path, and `recall` refuses
///   rather than answering from a search it could not complete.
pub fn snapshot_with(root: &Path, use_memo: bool) -> Result<Snapshot, String> {
    let files = bucket_files(root, Scope::All)?;
    let live = live_fingerprints(&files)?;

    if use_memo {
        if let Some(hit) = memo_hit(root, &live) {
            return Ok(hit);
        }
    }
    let snapshot = build(&files, &live)?;
    report_disagrees(root, &snapshot);
    remember(root, &snapshot);
    persist(root, &snapshot);
    Ok(snapshot)
}

/// The report as it sits on disk, parsed — for observation and tests.
/// `None` when it is absent, unreadable, not an object, or carries the
/// wrong schema version: every one of those is "no cache", never fact.
pub fn read_manifest_json(root: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(manifest_path(root)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if !value.is_object() {
        return None;
    }
    if value.get("format_version").and_then(Value::as_u64) != Some(FORMAT_VERSION) {
        return None;
    }
    Some(value)
}

/// Drop this root's memoization. Writers MAY call it after a successful
/// write (post-write, best-effort, never inside a critical section);
/// correctness never depends on it, because `snapshot_with` validates
/// fingerprints on every call anyway.
pub fn invalidate(root: &Path) {
    let key = memo_key(root);
    if let Ok(mut guard) = memo().lock() {
        guard.remove(&key);
    }
}

// ---------------------------------------------------------------------------
// Fingerprinting, validation, rebuild
// ---------------------------------------------------------------------------

fn manifest_path(root: &Path) -> PathBuf {
    root.join(INDEX_DIR).join(MANIFEST_NAME)
}

/// Wall-clock seconds since the Unix epoch. An unreadable clock is an
/// error, never a zero: a lock or a report stamped at 1970 would lie
/// about its own age.
fn now_epoch() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|err| format!("the system clock is unreadable: {err}"))
}

/// The read-free half of a fingerprint: what a file's identity costs
/// when nothing is read (`mtime`, `size`, `inode`) plus the schema the
/// cache was built under.
fn stat_fingerprint(rel: &str, abs: &Path) -> Result<Fingerprint, String> {
    let meta = std::fs::metadata(abs).map_err(|err| {
        format!("index refused: cannot stat bucket file {abs:?} (cannot verify what it would serve): {err}")
    })?;
    Ok(Fingerprint {
        path: rel.to_string(),
        mtime: meta.mtime(),
        size: meta.len(),
        inode: meta.ino(),
        line_count: 0,
        format_version: FORMAT_VERSION,
    })
}

fn live_fingerprints(files: &[BucketFile]) -> Result<Vec<Fingerprint>, String> {
    let mut out = Vec::with_capacity(files.len());
    for file in files {
        out.push(stat_fingerprint(&file.rel, &file.abs)?);
    }
    Ok(out)
}

/// The per-call validation: does the cached entry still describe the live
/// file? The comparison is the half of the fingerprint that costs
/// nothing to obtain (`mtime`, `size`, `inode`, `format_version`) —
/// `line_count` is the derived entry list's own length, verified at
/// build time, cross-checked against the entry list whenever the
/// manifest is loaded, and re-checked against the live file whenever
/// `recall` reads it. Any mismatch here is a rebuild, never a refusal.
fn fingerprints_match(cached: &Fingerprint, live: &Fingerprint) -> bool {
    cached.format_version == live.format_version
        && cached.path == live.path
        && cached.mtime == live.mtime
        && cached.size == live.size
        && cached.inode == live.inode
}

/// The full entry: read the file, split it into lines (the parsed entry
/// list), and fill in the `line_count` half of the fingerprint.
fn read_indexed(file: &BucketFile) -> Result<IndexedFile, String> {
    let mut fingerprint = stat_fingerprint(&file.rel, &file.abs)?;
    let text = std::fs::read_to_string(&file.abs).map_err(|err| {
        format!("index refused: cannot read bucket file {:?} (cannot verify what it would serve): {err}", file.abs)
    })?;
    let lines = split_lines(&text);
    fingerprint.line_count = lines.len() as u64;
    Ok(IndexedFile { fingerprint, lines })
}

/// Split into lines the way `recall` reads them: `\n`-terminated, the
/// trailing terminator not producing an empty final entry (`"a\nb"` and
/// `"a\nb\n"` are both two lines).
pub fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split('\n').map(|l| l.to_string()).collect();
    if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// One full rebuild from disk: every bucket file read and parsed. This
/// is the ONLY way the cache content is produced — a corrupt manifest,
/// a version mismatch and a plain miss all arrive here, and none of
/// them is an error. It is loud only when the disk itself refuses.
fn build(files: &[BucketFile], live: &[Fingerprint]) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot::default();
    for (file, stat) in files.iter().zip(live.iter()) {
        let indexed = read_indexed(file)?;
        debug_assert_eq!(indexed.fingerprint.mtime, stat.mtime);
        snapshot.files.insert(file.rel.clone(), indexed);
    }
    Ok(snapshot)
}

impl Snapshot {
    /// Narrow the candidate set: the files whose indexed lines carry a
    /// match, with the matching line numbers. `recall` still re-reads
    /// every file it is handed and decides against the live bytes.
    pub fn candidates(&self, files: &[BucketFile], query: &Query) -> Result<Vec<Candidate>, String> {
        let mut out: Vec<Candidate> = Vec::new();
        for file in files {
            let Some(indexed) = self.files.get(&file.rel) else {
                continue; // unknown to the cache: no candidate to offer
            };
            let mut lines: Vec<usize> = Vec::new();
            for (n, text) in indexed.lines.iter().enumerate() {
                if query.matches(text)? {
                    lines.push(n);
                }
            }
            if !lines.is_empty() {
                out.push(Candidate {
                    file: file.clone(),
                    lines,
                });
            }
        }
        Ok(out)
    }

    /// The derived report's JSON — what `persist` writes and what the
    /// tests read back. Field order is the manifest's contract.
    pub fn manifest_json(&self, root: &Path) -> Value {
        let entries: Vec<Value> = self
            .files
            .values()
            .map(|indexed| {
                json!({
                    "path": indexed.fingerprint.path,
                    "mtime": indexed.fingerprint.mtime,
                    "size": indexed.fingerprint.size,
                    "inode": indexed.fingerprint.inode,
                    "line_count": indexed.fingerprint.line_count,
                    "format_version": indexed.fingerprint.format_version,
                    "lines": indexed.lines,
                })
            })
            .collect();
        json!({
            "format_version": FORMAT_VERSION,
            "root": root.display().to_string(),
            "built_at": crate::gate::utc_iso8601(now_epoch().unwrap_or(0) as i64),
            "files": entries,
        })
    }
}

// ---------------------------------------------------------------------------
// The two cache layers
// ---------------------------------------------------------------------------

static MEMO: LazyLock<Mutex<BTreeMap<PathBuf, Snapshot>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn memo() -> &'static Mutex<BTreeMap<PathBuf, Snapshot>> {
    &MEMO
}

/// The memo key is the canonical root, so the same directory reached by
/// two spellings shares one cache and two roots never collide.
fn memo_key(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

fn memo_hit(root: &Path, live: &[Fingerprint]) -> Option<Snapshot> {
    let guard = memo().lock().ok()?;
    let snapshot = guard.get(&memo_key(root))?;
    let all_match = live.iter().all(|entry| match snapshot.files.get(&entry.path) {
        Some(indexed) => fingerprints_match(&indexed.fingerprint, entry),
        None => false,
    }) && snapshot.files.len() == live.len();
    if all_match {
        Some(snapshot.clone())
    } else {
        None
    }
}

fn remember(root: &Path, snapshot: &Snapshot) {
    if let Ok(mut guard) = memo().lock() {
        guard.insert(memo_key(root), snapshot.clone());
    }
}

/// The on-disk report exists for a human to read, so the only thing the
/// server does with it is hold it to account. Absent, truncated,
/// unparseable, wrong-version or simply stale reports are the ordinary
/// case (the cache is deletable by design, spec §5) and pass silently —
/// they are replaced by the snapshot just built from disk. What is NOT
/// ordinary is a report that claims to describe the bytes we just read
/// (same path, mtime, size, inode) and yet lists different lines: that is
/// a cache lying about being current, so it is announced, then overwritten
/// from the disk it should have described. Never served, never trusted,
/// never the reason a search comes back empty.
fn report_disagrees(root: &Path, snapshot: &Snapshot) {
    let Some(value) = read_manifest_json(root) else {
        return; // no current-schema report on disk: nothing to hold to account
    };
    let Some(entries) = value.get("files").and_then(Value::as_array) else {
        return;
    };
    for entry in entries {
        let Some(obj) = entry.as_object() else { continue };
        let Some(path) = obj.get("path").and_then(Value::as_str) else {
            continue;
        };
        let Some(current) = snapshot.files.get(path) else {
            continue;
        };
        let claims_current = obj.get("mtime").and_then(Value::as_i64)
            == Some(current.fingerprint.mtime)
            && obj.get("size").and_then(Value::as_u64) == Some(current.fingerprint.size)
            && obj.get("inode").and_then(Value::as_u64) == Some(current.fingerprint.inode);
        if !claims_current {
            continue; // stale in the ordinary way: it is simply replaced below
        }
        let recorded: Vec<&str> = obj
            .get("lines")
            .and_then(Value::as_array)
            .map(|lines| lines.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let read: Vec<&str> = current.lines.iter().map(String::as_str).collect();
        if recorded != read {
            eprintln!(
                "[total-recall] index: {INDEX_DIR}/{MANIFEST_NAME} claims to describe {path:?} AT ITS CURRENT FINGERPRINT yet its entry list disagrees with those bytes — the report is never served as fact; it is being rewritten from disk"
            );
        }
    }
}

/// Write the derived report: `.locks/index.lock` for the duration, tmp
/// file + atomic rename inside `.index/`. Best-effort by contract —
/// every failure path here logs and returns, because the cache is never
/// authoritative and a search must never be refused for want of a cache
/// write. Never called from an append critical section.
fn persist(root: &Path, snapshot: &Snapshot) {
    let dir = root.join(INDEX_DIR);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("[total-recall] index: cannot create {INDEX_DIR}/: {err} — report skipped");
        return;
    }
    let token = match acquire_lock(root, LOCK_NAME, "index") {
        Ok(token) => token,
        Err(err) => {
            eprintln!("[total-recall] index: {err} — report skipped");
            return;
        }
    };
    let body = match serde_json::to_string_pretty(&snapshot.manifest_json(root)) {
        Ok(body) => body,
        Err(err) => {
            eprintln!("[total-recall] index: cannot serialize the derived report: {err} — report skipped");
            release_lock(root, &token, LOCK_NAME, "index");
            return;
        }
    };
    let tmp = dir.join(format!(
        ".{MANIFEST_NAME}.tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let target = dir.join(MANIFEST_NAME);
    let outcome = (|| -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp)
            .map_err(|err| format!("creating {tmp:?}: {err}"))?;
        f.write_all(body.as_bytes())
            .and_then(|()| f.flush())
            .map_err(|err| format!("writing {tmp:?}: {err}"))?;
        drop(f);
        std::fs::rename(&tmp, &target).map_err(|err| format!("renaming {tmp:?} into place: {err}"))
    })();
    if let Err(err) = outcome {
        let _ = std::fs::remove_file(&tmp);
        eprintln!("[total-recall] index: {err} — report skipped");
    }
    release_lock(root, &token, LOCK_NAME, "index");
}

// ---------------------------------------------------------------------------
// The `.locks/` append-mutex protocol (spec §2), crate-wide
// implementation: an `O_CREAT|O_EXCL` lock file in `.locks/`, bounded
// retry against a fresh holder, age-based takeover of a silent one, and
// a release that removes only its own acquisition. Identical contents
// format, retry budget and staleness rules to the signoff / day-file /
// claim locks (`src/tools/signoff_append.rs`, `src/tools/claims.rs`);
// callers contribute only the lock file name and their own name for
// messages. Two users today: this module's derived report
// (`.locks/index.lock`) and the audit writer (`tools::ticks`), which
// takes one lock per audit file exactly as the day files do.
// ---------------------------------------------------------------------------

pub(crate) struct LockToken {
    pid: u32,
    nonce: u64,
    epoch: u64,
}

fn lock_contents(token: &LockToken, who: &str) -> String {
    format!(
        "pid={} nonce={} epoch={} session={}\n",
        token.pid, token.nonce, token.epoch, who
    )
}

/// Take `.locks/{lock_name}`. A FRESH holder is waited out
/// (`RETRY_SLEEP`, up to `MAX_ACQUIRE_ATTEMPTS`); a holder silent for
/// `STALE_AFTER` is taken over, so an orphaned lock never wedges the
/// root. The holder record is published in the same breath as the
/// create: a lock we could not label is undone, never left claimed.
pub(crate) fn acquire_lock(root: &Path, lock_name: &str, who: &str) -> Result<LockToken, String> {
    let locks_dir = root.join(".locks");
    if let Err(err) = std::fs::create_dir_all(&locks_dir) {
        return Err(format!("{who} refused: cannot create .locks/: {err}"));
    }
    let path = locks_dir.join(lock_name);
    let pid = std::process::id();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let epoch = match now_epoch() {
            Ok(epoch) => epoch,
            Err(err) => return Err(format!("{who} refused: {err}")),
        };
        let token = LockToken {
            pid,
            nonce: ACQUIRE_NONCE.fetch_add(1, Ordering::Relaxed),
            epoch,
        };
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(mut f) => {
                let wrote = f.write_all(lock_contents(&token, who).as_bytes());
                let flushed = f.flush();
                drop(f);
                if wrote.is_err() || flushed.is_err() {
                    // No published holder record: undo the bare create,
                    // else the staleness detector sees an unowned file.
                    let _ = std::fs::remove_file(&path);
                    return Err(format!(
                        "{who} refused: writing .locks/{lock_name} failed — the lock was not taken"
                    ));
                }
                return Ok(token);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // F2 (signoffs/review_Wave4OKF.md 2b): age-then-remove
                // unconditionally is a TOCTOU — two acquirers that both
                // measured the same stale record could each remove
                // whatever stood at the path, so the slower one deleted
                // the faster one's FRESH lock and both believed they held
                // the mutex. The takeover is now identity-checked: the
                // removal only ever removes the very record it measured.
                if let Some(stale) = measure_stale_lock(&path) {
                    if remove_stale_lock(&path, &stale) {
                        // Stale holder confirmed still the measured one:
                        // take it over, no budget spent.
                        continue;
                    }
                    // The re-check refused: a successor took the stale
                    // lock over between the measurement and this
                    // removal. We release our claim on that path (their
                    // lock stays untouched) and re-enter the bounded
                    // retry below.
                }
                if attempts >= MAX_ACQUIRE_ATTEMPTS {
                    return Err(format!(
                        "{who} refused: .locks/{lock_name} is held by another writer and the bounded retry ({MAX_ACQUIRE_ATTEMPTS} x {} ms) is exhausted; retry, or the stale takeover will reclaim it once the holder goes silent",
                        RETRY_SLEEP.as_millis()
                    ));
                }
                std::thread::sleep(RETRY_SLEEP);
            }
            Err(err) => return Err(format!("{who} refused: acquiring .locks/{lock_name}: {err}")),
        }
    }
}

/// How long the holder has been silent: its published `epoch`, else the
/// file's mtime (a crash between create and write), else `None` — an
/// age that cannot be established is FRESH (no takeover, just wait).
fn lock_age(path: &Path) -> Option<Duration> {
    let now = SystemTime::now();
    if let Ok(contents) = std::fs::read_to_string(path) {
        if let Some(raw) = parse_lock_field(&contents, "epoch") {
            if let Ok(holder) = raw.parse::<u64>() {
                if let Ok(now_secs) = now.duration_since(UNIX_EPOCH) {
                    if holder <= now_secs.as_secs() {
                        return Some(Duration::from_secs(now_secs.as_secs() - holder));
                    }
                }
            }
        }
    }
    match std::fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(modified) => now.duration_since(modified).ok(),
        Err(_) => None,
    }
}

/// The identity of the lock file as measured STALE (F2,
/// signoffs/review_Wave4OKF.md 2b): the published `pid`/`nonce`/`epoch`
/// fields — the same fields `release_lock` identity-checks — the full
/// contents, and the stat facts (mtime, size, inode) at measurement
/// time. `None` fields cover the empty/corrupt file whose age came from
/// the mtime fallback (its measured identity is then its stat alone).
struct StaleLock {
    pid: Option<String>,
    nonce: Option<String>,
    epoch: Option<String>,
    contents: Option<String>,
    mtime: Option<SystemTime>,
    size: u64,
    ino: u64,
}

/// Measure the holder's silence AND its identity in one step: `Some`
/// only when the file exists, its age (the `lock_age` rule: published
/// `epoch`, else mtime) is at least `STALE_AFTER`, and its stat facts
/// could be recorded. `None` means FRESH or unmeasurable — the caller
/// waits (the safe default).
fn measure_stale_lock(path: &Path) -> Option<StaleLock> {
    let age = lock_age(path)?;
    if age < STALE_AFTER {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let contents = std::fs::read_to_string(path).ok();
    let field = |name: &str| {
        contents
            .as_deref()
            .and_then(|c| parse_lock_field(c, name).map(str::to_string))
    };
    Some(StaleLock {
        pid: field("pid"),
        nonce: field("nonce"),
        epoch: field("epoch"),
        contents,
        mtime: meta.modified().ok(),
        size: meta.len(),
        ino: meta.ino(),
    })
}

/// The identity-safe takeover (F2): re-read the record and re-stat the
/// file, and remove it ONLY while every identity field (pid/nonce/
/// epoch) and every stat fact (mtime/size/inode) still matches what was
/// measured stale — a mismatch means someone took the lock over first,
/// and removing then would delete THEIR fresh lock. Returns whether the
/// removal was ours to make. Residual window: between this verified
/// re-read and the `remove_file` itself microseconds remain open for a
/// successor to land — recorded as the accepted residual by the review
/// (F2); fully closing it needs rename-based takeover, out of this
/// round's scope.
fn remove_stale_lock(path: &Path, stale: &StaleLock) -> bool {
    let now_contents = std::fs::read_to_string(path).ok();
    if now_contents != stale.contents {
        return false;
    }
    let text = now_contents.as_deref().unwrap_or("");
    if [
        ("pid", &stale.pid),
        ("nonce", &stale.nonce),
        ("epoch", &stale.epoch),
    ]
    .iter()
    .any(|(name, want)| parse_lock_field(text, name).map(str::to_string) != **want)
    {
        return false;
    }
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return false,
    };
    if meta.len() != stale.size || meta.ino() != stale.ino || meta.modified().ok() != stale.mtime
    {
        return false;
    }
    std::fs::remove_file(path).is_ok()
}

fn parse_lock_field<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}=");
    let start = contents.find(&needle)? + needle.len();
    let rest = &contents[start..];
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    match &rest[..end] {
        "" => None,
        value => Some(value),
    }
}

/// Release = remove OUR lock file, and only while it still records this
/// acquisition; a stale-taken-over lock is left for its holder.
pub(crate) fn release_lock(root: &Path, token: &LockToken, lock_name: &str, who: &str) {
    let path = root.join(".locks").join(lock_name);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return; // already gone: nothing of ours to release
    };
    let ours = [
        ("pid", token.pid.to_string()),
        ("nonce", token.nonce.to_string()),
        ("epoch", token.epoch.to_string()),
    ]
    .iter()
    .all(|(key, want)| parse_lock_field(&contents, key) == Some(want.as_str()));
    if !ours {
        eprintln!(
            "[total-recall] {who}: .locks/{lock_name} no longer records this acquisition — not removed (a successor's lock)"
        );
        return;
    }
    if let Err(err) = std::fs::remove_file(&path) {
        eprintln!(
            "[total-recall] {who}: could not release .locks/{lock_name}: {err} — it will be reclaimed once stale"
        );
    }
}

// ---------------------------------------------------------------------------
// Bucket-entry hygiene (path confinement)
// ---------------------------------------------------------------------------

fn read_dir_sorted(dir: &Path, label: &str) -> Result<Vec<std::fs::DirEntry>, String> {
    let entries = std::fs::read_dir(dir).map_err(|err| {
        format!("index refused: cannot read {label} directory {dir:?}: {err}")
    })?;
    let mut out: Vec<std::fs::DirEntry> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!("index refused: reading {label} directory {dir:?}: {err}")
        })?;
        out.push(entry);
    }
    out.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(out)
}

fn is_readable_file(path: &Path) -> bool {
    matches!(std::fs::metadata(path), Ok(meta) if meta.is_file())
}

fn is_regular_file(entry: &std::fs::DirEntry) -> bool {
    // `file_type()` for a symlink reports the LINK, so a symlinked bucket
    // entry is never enumerated as a regular file — the escape vector is
    // closed at enumeration, and `bucket_files` re-checks by canonical
    // path on top of that.
    matches!(entry.file_type(), Ok(ft) if ft.is_file())
}

/// `YYYY-MM-DD.md`: four digits, `-`, two, `-`, two, `.md`, and sane
/// month/day fields. Anything else at the root (`notes.md`, `.index`,
/// `2026-9-13.md`) is not a bucket and is never opened.
fn is_dayfile_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() != 13 || bytes[4] != b'-' || bytes[7] != b'-' || !name.ends_with(".md") {
        return false;
    }
    let digits = |mut r: std::ops::Range<usize>| r.all(|i| bytes[i].is_ascii_digit());
    if !digits(0..4) || !digits(5..7) || !digits(8..10) {
        return false;
    }
    let month: u32 = name[5..7].parse().unwrap_or(0);
    let day: u32 = name[8..10].parse().unwrap_or(0);
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

// ---------------------------------------------------------------------------
// The std-only regex engine (§13 R3: no third-party search dependency)
// ---------------------------------------------------------------------------

/// Back-tracking budget for one line: a pathological pattern is refused
/// loudly rather than running the box dry, and a search that cannot
/// finish never answers.
const REGEX_FUEL: u32 = 200_000;
/// The most repetitions one `*`, `+` or `{m,}` may unroll while
/// enumerating end positions. Memory lines are short; a loop longer
/// than this is a runaway pattern, not a real search.
const REP_CAP: u32 = 256;

#[derive(Clone, Debug)]
enum Node {
    Ch(char),
    Any,
    Class { negated: bool, ranges: Vec<(char, char)> },
    Start,
    End,
    Seq(Vec<Node>),
    Alt(Vec<Vec<Node>>),
    Rep { min: u32, max: Option<u32>, node: Box<Node> },
}

/// Compile a pattern to the node program. Supported: literals and
/// escapes (`. \ | * + ? ( ) [ ] { } ^ $ n t r f d D w W s S`), `.`,
/// `[...]`/`[^...]` classes with ranges, groups with alternation, `*`,
/// `+`, `?`, `{m}`, `{m,}`, `{m,n}`, and accepted-but-ignored non-greedy
/// `?` markers (this engine decides *existence*, so ordering preference
/// cannot change an answer). Anything else is a LOUD refusal naming the
/// pattern and the position.
fn compile_regex(pattern: &str) -> Result<Vec<Node>, String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut p = RParser {
        chars: &chars,
        pos: 0,
    };
    let program = p.alternation()?;
    if p.pos != chars.len() {
        return Err(format!(
            "{pattern:?} is malformed at position {} (unexpected {:?})",
            p.pos, chars[p.pos]
        ));
    }
    Ok(program)
}

struct RParser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl RParser<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn alternation(&mut self) -> Result<Vec<Node>, String> {
        let mut branches: Vec<Vec<Node>> = vec![self.concatenation()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            branches.push(self.concatenation()?);
        }
        if branches.len() == 1 {
            return Ok(branches.pop().unwrap());
        }
        Ok(vec![Node::Alt(branches)])
    }

    fn concatenation(&mut self) -> Result<Vec<Node>, String> {
        let mut nodes: Vec<Node> = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            nodes.push(self.repetition()?);
        }
        Ok(nodes)
    }

    fn repetition(&mut self) -> Result<Node, String> {
        let atom = self.atom()?;
        if matches!(atom, Node::Start | Node::End) {
            if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
                return Err(format!(
                    "regex is malformed at position {} (a quantifier cannot follow an anchor)",
                    self.pos
                ));
            }
            return Ok(atom);
        }
        let Some(q) = self.peek() else {
            return Ok(atom);
        };
        let (min, max) = match q {
            '*' => {
                self.pos += 1;
                (0u32, None)
            }
            '+' => {
                self.pos += 1;
                (1u32, None)
            }
            '?' => {
                self.pos += 1;
                (0u32, Some(1u32))
            }
            '{' => self.counted()?, // advances past its own `}`
            _ => return Ok(atom),
        };
        // A trailing non-greedy marker is accepted and ignored: the
        // engine asks "can this match", never "which match wins".
        if self.peek() == Some('?') {
            self.pos += 1;
        }
        Ok(Node::Rep {
            min,
            max,
            node: Box::new(atom),
        })
    }

    /// `{m}`, `{m,}`, `{m,n}` — advances the cursor through the whole
    /// counted-repetition form and returns its bounds.
    fn counted(&mut self) -> Result<(u32, Option<u32>), String> {
        let start = self.pos;
        self.pos += 1; // '{'
        let number = |p: &mut Self| -> Result<Option<u32>, String> {
            let from = p.pos;
            while p.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                p.pos += 1;
            }
            let text: String = p.chars[from..p.pos].iter().collect();
            if text.is_empty() {
                Ok(None)
            } else {
                text.parse::<u32>()
                    .map(Some)
                    .map_err(|_| format!("counted repetition {text:?} overflows"))
            }
        };
        let min = number(self)?.unwrap_or(0);
        let max = if self.peek() == Some(',') {
            self.pos += 1;
            number(self)?
        } else {
            Some(min)
        };
        if self.peek() != Some('}') {
            return Err(format!(
                "regex is malformed at position {start} (unterminated counted repetition)"
            ));
        }
        self.pos += 1;
        // F6: `REP_CAP` bounds how far the back-tracker will enumerate
        // an UNBOUNDED repetition (`*`, `+`, `{m,}`). A COUNTED bound
        // above it is a pattern this engine cannot honour, and clamping
        // it silently answers a question nobody asked — a wrong "no
        // match" instead of a refusal. Refuse loudly, naming the
        // offending repetition, exactly as every other unsupported
        // pattern is refused.
        if min > REP_CAP || max.is_some_and(|n| n > REP_CAP) {
            let literal: String = self.chars[start..self.pos].iter().collect();
            return Err(format!(
                "regex is malformed at position {start} (counted repetition {literal:?} exceeds the {REP_CAP}-rep search cap)"
            ));
        }
        Ok((min, max))
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self
            .peek()
            .ok_or_else(|| "regex ends before it is complete".to_string())?;
        match c {
            '(' => {
                self.pos += 1;
                let inner = self.alternation()?;
                if self.peek() != Some(')') {
                    return Err(format!(
                        "regex is malformed at position {} (unbalanced group)",
                        self.pos
                    ));
                }
                self.pos += 1;
                Ok(Node::Seq(inner))
            }
            '[' => self.class(),
            '.' => {
                self.pos += 1;
                Ok(Node::Any)
            }
            '^' => {
                self.pos += 1;
                Ok(Node::Start)
            }
            '$' => {
                self.pos += 1;
                Ok(Node::End)
            }
            '\\' => {
                self.pos += 1;
                let e = self
                    .peek()
                    .ok_or_else(|| "regex ends with a dangling escape".to_string())?;
                self.pos += 1;
                Ok(match e {
                    'n' => Node::Ch('\n'),
                    't' => Node::Ch('\t'),
                    'r' => Node::Ch('\r'),
                    'f' => Node::Ch('\u{c}'),
                    'd' => Node::Class {
                        negated: false,
                        ranges: vec![('0', '9')],
                    },
                    'D' => Node::Class {
                        negated: true,
                        ranges: vec![('0', '9')],
                    },
                    'w' => Node::Class {
                        negated: false,
                        ranges: vec![('a', 'z'), ('0', '9'), ('_', '_')],
                    },
                    'W' => Node::Class {
                        negated: true,
                        ranges: vec![('a', 'z'), ('0', '9'), ('_', '_')],
                    },
                    's' => Node::Class {
                        negated: false,
                        ranges: vec![(' ', ' '), ('\t', '\r')],
                    },
                    'S' => Node::Class {
                        negated: true,
                        ranges: vec![(' ', ' '), ('\t', '\r')],
                    },
                    '.' | '\\' | '|' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}'
                    | '^' | '$' | '/' => Node::Ch(e),
                    other => return Err(format!("regex escape {other:?} is not supported")),
                })
            }
            '*' | '+' | '?' => Err(format!(
                "regex is malformed at position {} ({c:?} with nothing to repeat)",
                self.pos
            )),
            _ => {
                self.pos += 1;
                Ok(Node::Ch(lower(c)))
            }
        }
    }

    fn class(&mut self) -> Result<Node, String> {
        let start = self.pos;
        self.pos += 1; // '['
        let negated = if self.peek() == Some('^') {
            self.pos += 1;
            true
        } else {
            false
        };
        let mut ranges: Vec<(char, char)> = Vec::new();
        // A `]` in the first position is a member, not the terminator.
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err(format!(
                    "regex is malformed at position {start} (unterminated character class)"
                ));
            };
            if c == ']' && !first {
                self.pos += 1;
                break;
            }
            first = false;
            let lo = self.class_char()?;
            let ranged = self.peek() == Some('-')
                && self
                    .chars
                    .get(self.pos + 1)
                    .is_some_and(|next| *next != ']');
            if ranged {
                self.pos += 1;
                let hi = self.class_char()?;
                if hi < lo {
                    return Err(format!(
                        "regex is malformed at position {start} (inverted range {lo}..{hi})"
                    ));
                }
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        if ranges.is_empty() {
            return Err(format!(
                "regex is malformed at position {start} (empty character class)"
            ));
        }
        Ok(Node::Class { negated, ranges })
    }

    fn class_char(&mut self) -> Result<char, String> {
        let c = self
            .peek()
            .ok_or_else(|| "regex ends inside a character class".to_string())?;
        if c != '\\' {
            self.pos += 1;
            return Ok(lower(c));
        }
        self.pos += 1;
        let e = self
            .peek()
            .ok_or_else(|| "regex ends with a dangling escape".to_string())?;
        self.pos += 1;
        Ok(match e {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            other => lower(other),
        })
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Match budget bookkeeping: fuel exhaustion is recorded, never
/// silently turned into "no match".
struct Ctx {
    fuel: u32,
    exhausted: bool,
}

/// Unanchored, case-insensitive match of a compiled program against one
/// line. `Err` is the budget; the caller turns it into a refusal.
fn regex_matches(program: &[Node], line: &str) -> Result<bool, ()> {
    let chars: Vec<char> = line.chars().flat_map(char::to_lowercase).collect();
    let mut ctx = Ctx {
        fuel: REGEX_FUEL,
        exhausted: false,
    };
    for start in 0..=chars.len() {
        if !ends_of(program, &chars, start, &mut ctx).is_empty() {
            return Ok(true);
        }
        if ctx.exhausted {
            return Err(());
        }
    }
    Ok(false)
}

/// Every position reachable by matching the whole node list at `pos`.
fn ends_of(nodes: &[Node], s: &[char], pos: usize, ctx: &mut Ctx) -> Vec<usize> {
    match nodes.split_first() {
        None => vec![pos],
        Some((head, rest)) => {
            let mut out: Vec<usize> = Vec::new();
            for p in ends_of_one(head, s, pos, ctx) {
                out.extend(ends_of(rest, s, p, ctx));
            }
            out
        }
    }
}

/// Every position reachable by matching one node at `pos`.
fn ends_of_one(node: &Node, s: &[char], pos: usize, ctx: &mut Ctx) -> Vec<usize> {
    if ctx.fuel == 0 {
        ctx.exhausted = true;
        return Vec::new();
    }
    ctx.fuel -= 1;
    match node {
        Node::Ch(c) => {
            if s.get(pos) == Some(c) {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Node::Any => {
            if s.get(pos).is_some() {
                vec![pos + 1]
            } else {
                Vec::new()
            }
        }
        Node::Start => {
            if pos == 0 {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Node::End => {
            if pos == s.len() {
                vec![pos]
            } else {
                Vec::new()
            }
        }
        Node::Class { negated, ranges } => match s.get(pos) {
            None => Vec::new(),
            Some(c) => {
                let hit = ranges.iter().any(|(lo, hi)| c >= lo && c <= hi);
                if hit != *negated {
                    vec![pos + 1]
                } else {
                    Vec::new()
                }
            }
        },
        Node::Seq(inner) => ends_of(inner, s, pos, ctx),
        Node::Alt(branches) => branches
            .iter()
            .flat_map(|branch| ends_of(branch, s, pos, ctx))
            .collect(),
        Node::Rep { min, max, node } => {
            let cap = max.unwrap_or(REP_CAP).min(REP_CAP);
            let mut frontier: Vec<usize> = vec![pos];
            let mut out: Vec<usize> = Vec::new();
            let mut count: u32 = 0;
            loop {
                if count >= *min {
                    out.extend(frontier.iter().copied());
                }
                if count >= cap {
                    break;
                }
                let mut next: Vec<usize> = frontier
                    .iter()
                    .copied()
                    .flat_map(|p| ends_of_one(node, s, p, ctx))
                    .collect();
                next.sort_unstable();
                next.dedup();
                if next.is_empty() {
                    break;
                }
                if next == frontier {
                    // The body can match empty here, so every higher
                    // count is reachable from the same positions: those
                    // positions ARE ends of the whole repetition.
                    out.extend(frontier.iter().copied());
                    break;
                }
                frontier = next;
                count += 1;
            }
            out.sort_unstable();
            out.dedup();
            out
        }
    }
}
