# Wins Log - Total-Recall Implementation

## 2026-03-07 - Dependency Conflicts Resolved

### Issue 1: ort-extras crate not found
**Problem:** The `ort-extras` crate was not available in crates.io

**Solution:** Removed `ort-extras` dependency - it was not being used in the implementation. The `ort` crate alone provides ONNX runtime functionality.

### Issue 2: ort feature `download-prebuilt-binaries` doesn't exist
**Problem:** Attempting to use `features = ["download-prebuilt-binaries"]` with `ort` crate

**Solution:** Removed the non-existent feature flag. The `ort` crate handles prebuilt binaries internally.

### Issue 3: rusqlite `vfs` feature not available
**Problem:** The `rusqlite` crate v0.34 doesn't have a `vfs` feature

**Solution:** Removed `vfs` feature. File I/O is handled separately via standard library, not SQLite VFS.

### Issue 4: pragmagh crate removed
**Problem:** The `pragmagh` crate was not being used anywhere in the codebase

**Solution:** Removed unused crate dependency.

### Issue 5: SQLite version chain conflicts with deadpool-sqlite
**Problem:** Multiple versions of `libsqlite3-sys` being pulled in by different rusqlite/deadpool-sqlite versions causing `links = "sqlite3"` conflicts

**Solution:** 
1. Removed `deadpool-sqlite` entirely - it wasn't being used in the code
2. Downgraded `rusqlite` to v0.30 which works with the bundled SQLite

### Issue 6: Rust version incompatibility with ort
**Problem:** `ort 2.0.0-rc.12` requires Rust 1.88, but system has Rust 1.85.1

**Potential solutions:**
1. Upgrade Rust to 1.88+ (`rustup update`)
2. Use an older version of ort that supports Rust 1.85
3. Continue with hash-based embeddings (current placeholder) without ONNX

**Decision:** Hash-based embeddings working as placeholder. ONNX integration can be added once Rust is upgraded.

## 2026-03-07 - Date Format Standardization

### Issue: Date format mismatch between plan and implementation
**Problem:** Implementation used `mm/dd/yyyy` (slash-separated) but plan specifies `mm-dd-yyyy` (dash-separated)

**Solution:** Updated `main.rs` to use `%m-%d-%Y` format string

## 2026-03-07 - Vector Database Engine Selection

### Issue: Choice of vector database backend
**Problem:** Implementation used FLEXVEC but plan specifies SQLite VSS extension

**Solution:** 
1. Changed to `CREATE VIRTUAL TABLE ... USING vss()` syntax
2. Added runtime VSS extension loading via `conn.load_extension("sqlite3_vss")`

## 2026-03-07 - Rust Version Upgrade - SUCCESS

### Issue: Rust version incompatibility with ort ONNX crate
**Problem:** `ort 2.0.0-rc.12` requires Rust 1.88, but system had Rust 1.85.1

**Action:** Executed `rustup update stable`

**Result:** Successfully upgraded from Rust 1.85.1 to 1.94.0 (released 2026-03-05)

**Resolution:** ONNX integration can now proceed. Hash-based embedding placeholder no longer needed as a blocker.

## 2026-03-07 - License Alignment - CORRECTED

### Issue: LICENSE file (GPL v2) vs Cargo.toml (MIT) mismatch
**Problem (Initial):** The LICENSE file contained GNU GPL v2 text, but Cargo.toml declared `license = "MIT"`

**Problem (User Correction):** License should remain GPL-2.0 as per original LICENSE file, not changed to MIT.

**Initial Incorrect Action:** Replaced GPL v2 LICENSE with MIT license text (WRONG - this violated user's license choice).

**Corrective Action:** 
1. Restored original GPL-2.0 license text from GNU official source
2. Updated Cargo.toml to `license = "GPL-2.0"` to match LICENSE file

**Result:** License now consistent - both LICENSE file and Cargo.toml specify GPL-2.0

**Lesson:** Always confirm license preferences with user before making changes. The original LICENSE file already had the correct GPL-2.0 license - no replacement was necessary, just alignment of Cargo.toml.
