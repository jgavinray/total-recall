# TR-4 — Set up sqlite-vec extension for vector search

**Repo:** `/Users/jgavinray/dev/total-recall`

## Problem Statement
`build.rs` detects `vendor/sqlite-vec` is missing and warns:
```
warning: sqlite-vec not found, preparing for manual installation
warning: Please run: git clone https://github.com/asg017/sqlite-vec vendor/sqlite-vec
```

Additionally, `store.rs` calls `conn.load_extension("sqlite3_vss")` but rusqlite's `load_extension` method requires the `load_extension` feature flag in Cargo.toml (currently not enabled).

Without sqlite-vec, vector similarity search (`search_notes` tool) is non-functional.

## Suggested Approach

**Option A (preferred — try first):** Use `sqlite-vec` crate from crates.io which bundles the extension. Check if it exists and works with rusqlite 0.30:
- Search crates.io or check `cargo add sqlite-vec`
- If it exists, add to Cargo.toml and wire it up in store.rs

**Option B (fallback):**
1. Add `load_extension` feature to rusqlite in Cargo.toml: `rusqlite = { version = "0.30", features = ["bundled", "load_extension"] }`
2. Clone sqlite-vec: `cd /Users/jgavinray/dev/total-recall && git clone https://github.com/asg017/sqlite-vec vendor/sqlite-vec`
3. Update `build.rs` to compile sqlite-vec using `cc` crate (already in `[build-dependencies]`)
4. Update `store.rs` to load extension correctly

## Acceptance Criteria
1. `cargo build` produces no sqlite-vec warning
2. Vector extension loads without compile errors
3. Vector virtual table can be created (verify the extension initializes in store.rs init)
4. Conventional commit with proper body (Problem/Solution/Notes)
5. PATCH board to in-review when done

## Constraints
- NO new npm dependencies (Rust/Cargo deps are fine)
- Repo is at `/Users/jgavinray/dev/total-recall`
- When done, PATCH the Kanban board
