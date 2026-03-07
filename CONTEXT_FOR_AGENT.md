# Total-Recall: Agentic Memory MCP Server
## Agent Execution Context

**This document consolidates all critical information needed to execute the Total-Recall implementation.**

Feed this single file to assist with understanding constraints, build procedures, and execution flow. All detailed specifications reference `plan.md` and full implementation code references `implementation.md`.

---

## Quick Reference

### Project Goal
Build a Rust-based agentic memory system that:
- ✅ Stores notes as dated markdown files (`mm-dd-yyyy.md` format)
- ✅ Uses SQLite + ONNX embeddings for semantic search
- ✅ Implements MCP protocol with 5 tools
- ✅ Files are immutable - never overwrite, only append/archived

### File Organization
```
~/.total-recall/                    # Default memory directory (configurable)
├── 03-2026/                        # mm-yyyy/ subdirectory
│   ├── 03-06-2026.md               # mm-dd-yyyy.md filename
│   ├── 03-05-2026.md
│   └── ...
├── 02-2026/
│   └── ...
├── memory.db                       # SQLite index with vector embeddings
└── config.yaml                     # Configuration file
```

### MCP Tools (5 total)
| Tool | Description |
|------|-------------|
| `write_note` | Create new note (error if date exists - immutability) |
| `read_note` | Read note by date (`mm-dd-yyyy`) |
| `search_notes` | Vector semantic search |
| `recent_notes` | Notes from last N days |
| `build_context` | Observations from specific date |

---

## Environment & Build Requirements

### Prerequisites
- **Rust**: 1.85+ (`rustc --version`, `cargo --version`)
- **SQLite**: 3.40+ with build tools (`sqlite3 --version`, `gcc/clang`)
- **Platform**: macOS arm64/x86_64, Linux x86_64 (test on your target first)

### Dependencies (Cargo.toml)
```toml
[dependencies]
mcp = "0.12"
tokio = { version = "1.46", features = ["full"] }
rusqlite = { version = "0.34", features = ["bundled"] }
ort = "2.0"                    # ONNX runtime
serde = { version = "1.0", features = ["derive"] }
serde_yaml = "0.9"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4.5", features = ["derive"] }
thiserror = "2.0"
tracing = "0.1"
ndarray = "0.16"
async-trait = "0.1"
dirs = "6.0"
```

### Build Script (build.rs)
`build.rs` compiles **sqlite-vec C extension** at build time:
```rust
fn main() {
    // Download sqlite-vec source from GitHub
    // Compile as C library
    // Link to main binary
}
```

**Critical:** Build must succeed before any Rust code compiles.

---

## Execution Flow - Step by Step

### Phase 0: Setup
```bash
cd /Users/jgavinray/dev/memory/total-recall
cargo build --release
```
**Pass criteria:** No compilation errors, sqlite-vec builds successfully

### Phase 1: Core Modules (in order)
1. **Error handling** (`src/error.rs`)
2. **Data models** (`src/memory/models.rs`)
3. **File parser** (`src/memory/file_parser.rs`)
4. **SQLite store** (`src/memory/store.rs`)
5. **Embedder** (`src/memory/embedder.rs`)

After EACH module, run tests:
```bash
cargo test --test <module>_tests
```

### Phase 2: MCP Server
```bash
cargo build --release
```
**Pass criteria:** Binary runs, connects via stdio, lists tools correctly

### Phase 3: Integration
- Write test notes using `write_note` tool
- Verify search returns correct semantic matches
- Test archive/restore flow

---

## Architecture Reference

See `plan.md` for:
- Full architecture diagram
- Database schema (notes + observations tables)
- Observation categories (decision, action, note, idea, question, risk)
- Vector indexing details

### Database Schema
```sql
CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    date TEXT NOT NULL UNIQUE,    -- mm-dd-yyyy
    title TEXT,
    content TEXT NOT NULL,
    created_at INTEGER,
    updated_at INTEGER,
    archived INTEGER DEFAULT 0
);

CREATE TABLE observations (
    id TEXT PRIMARY KEY,
    note_id TEXT NOT NULL,
    timestamp TEXT,
    category TEXT,
    content TEXT NOT NULL,
    embedding FLOAT[384]          -- ONNX MiniLM embedding
);
```

---

## Implementation Flow Reference

See `implementation.md` for:
- Complete code templates for each step
- Unit test code for validation
- Acceptance criteria for each phase

### Key Implementation Notes

1. **Date Format**: Always use `mm-dd-yyyy` (e.g., `03-06-2026` for March 6th)
2. **Immutability**: `write_note` fails if date exists; use soft delete (archive) instead
3. **Observation Parsing**: Extract `- [category] content #tag` lines
4. **Embeddings**: 384-dim vectors from `all-MiniLM-L6-v2` ONNX model
5. **Config**: Load from `config.yaml` at startup (see `src/config.rs`)

---

## Testing Protocol

### Unit Tests
```bash
cargo test --test error_tests
cargo test --test models_tests
cargo test --test parser_tests
cargo test --test store_tests
cargo test --test embedder_tests
```

### Integration Tests
Run full test suite after MCP implementation:
```bash
cargo test --release
```

### Acceptance Criteria
- All unit tests pass (15+ test files)
- Binary builds without warnings
- Can write/read/search notes via MCP stdio
- Embeddings produce consistent results for same input

---

## Configuration Reference

See `config.yaml.example` for full options.

**Critical config values:**
```yaml
memory_dir: ~/.total-recall       # Where markdown files live
db_path: ~/.total-recall/memory.db  # SQLite index
embedding.model: sentence-transformers/all-MiniLM-L6-v2
```

CLI override:
```bash
total-recall serve --config /path/to/config.yaml
```

---

## Common Issues & Solutions

### Issue: sqlite-vec fails to build
**Cause**: Missing C compiler or SQLite dev headers  
**Fix**: `brew install sqlite3` (macOS) or `apt-get install libsqlite3-dev` (Linux)

### Issue: ONNX model not found
**Cause**: Model download failed or cache corrupted  
**Fix**: Delete `~/.total-recall/models/` and restart

### Issue: File exists error on write
**Cause**: Immutability enforcement  
**Fix**: Use `archive_note` instead, or pick new date

---

## Success Criteria Checklist

- [ ] Project compiles with `cargo build --release`
- [ ] All 15+ test files pass
- [ ] Binary can serve via MCP stdio transport
- [ ] 5 MCP tools respond correctly to requests
- [ ] Vector search returns meaningful semantic results
- [ ] Archive/restore works without data loss

---

## Document References

| Document | Purpose |
|----------|---------|
| `plan.md` | Architecture, MCP specs, database schema |
| `implementation.md` | Step-by-step code with tests |
| `config.yaml.example` | Configuration template |
| `src/config.rs` | Config parsing implementation |
| `src/memory/models.rs` | Note/Observation data types |
| `build.rs` | sqlite-vec build script |

---

## Next Actions for Agent

1. Review `plan.md` for architecture understanding
2. Follow `implementation.md` step-by-step
3. Build sqlite-vec successfully in Phase 0
4. Run all tests after each step
5. Verify binary works via MCP stdio

**Start with:** `cargo build --release` to ensure environment is correct.
