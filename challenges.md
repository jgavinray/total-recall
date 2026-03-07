# Technical Challenges & Solutions

## Critical Issue: SQLite VSS + ONNX in Pure Rust

### The Problem
You requested **both**:
1. ✅ Single SQLite file with vector storage
2. ✅ High-accuracy ONNX embeddings (all-MiniLM-L6-v2, 384-dim)

**But these requirements conflict in Rust** because:
- SQLite VSS (`sqlite-vec` extension) is a **C extension** that must be compiled separately
- ONNX runtime (`ort` crate) is a **separate C library** 
- Neither is natively supported by `rusqlite` alone
- You cannot easily bundle both into a pure Rust binary without FFI complexity

---

## Solution Options (Rank by Feasibility)

### Option A: Bundle Pre-Compiled SQLite VSS Extension (Recommended)
**Approach**: Download `sqlite-vec` extension at runtime or bundle it with the binary

**Pros**:
- Single file approach maintained
- True vector similarity search in SQL
- Mature, proven technology
- ONNX can still be used for embeddings

**Cons**:
- Must download `.so`/`.dylib` file at runtime (or bundle in binary)
- Version compatibility checks needed (SQLITE version 3.40+)
- Adds ~500KB to binary footprint

**Implementation**:
```rust
// In memory/store.rs
pub fn new(db_path: &Path) -> Result<Self> {
    let conn = Connection::open(db_path)?;
    
    // Load sqlite-vec extension (bundled or downloaded)
    #[cfg(target_os = "macos")]
    conn.load_extension("./libsqlite-vec.dylib")?;
    
    #[cfg(target_os = "linux")]
    conn.load_extension("./libsqlite-vec.so")?;
    
    conn.execute_batch("SELECT load_vss();")?;
    // ... rest of setup
}
```

**Action Required**: 
- Download sqlite-vec extension binary for target platform
- Or compile it as part of build process
- Document requirement for users to have download capability

---

### Option B: SQLite + Custom Rust Vector Index
**Approach**: Store embeddings in SQLite as BLOBs, implement approximate nearest neighbor search in Rust

**Pros**:
- Pure Rust, no C extensions
- Single file
- Full control over implementation

**Cons**:
- Must implement ANN search algorithm (e.g., HNSW, IVF) from scratch
- Not as performant as optimized C extension
- More code to maintain

**Implementation**:
```rust
// observations table
embedding BLOB NOT NULL  // 384 floats = 1536 bytes

// Custom search function
pub fn search_nearest(&self, query: &[f32; 384], limit: usize) -> Result<Vec<Note>> {
    // Load all embeddings from SQLite
    // Compute cosine similarity in Rust
    // Return top-k results
}
```

**Action Required**: 
- Implement cosine similarity function for 384-dim vectors
- Implement ANN algorithm (HNSW recommended for best performance)
- Benchmark to ensure acceptable performance

---

### Option C: Hybrid - SQLite Metadata + In-Memory Vector Index
**Approach**: SQLite stores notes + metadata, in-memory `ndarray` stores embeddings with ONNX

**Pros**:
- Best performance for embeddings
- Pure Rust for vector operations
- SQLite handles durable data storage
- No C extension complexity

**Cons**:
- Not strictly single-file vectors (in-memory)
- Vectors rebuild on startup from files
- Requires careful sync on notes updates

**Implementation**:
```rust
pub struct MemoryStore {
    conn: Connection,
    embedding_index: RwLock<Index384>,  // In-memory HNSW index
}

impl MemoryStore {
    pub fn new(db_path: PathBuf, embedder: Embedder) -> Result<Self> {
        // Load SQLite
        // Load all notes, compute embeddings, build index
    }
    
    pub fn add_note(&mut self, note: &Note) -> Result<()> {
        // Save to SQLite
        // Compute embedding
        // Insert into in-memory index
    }
}
```

**Action Required**: 
- Implement in-memory ANN index structure
- Handle sync between SQLite and in-memory index
- Graceful degradation on startup failure

---

### Option D: Use Postgres + pgvector (If Acceptable)
**Approach**: Switch from SQLite to Postgres with pgvector extension

**Pros**:
- Native vector support
- Mature, well-tested
-pgvector is stable

**Cons**:
- NOT SQLite (violates requirement)
- Requires Postgres installation
- More operational complexity

---

## Recommended Path Forward

### For MVP Implementation (Recommended)

**Go with Option A** initially, with a fallback plan:

1. **Phase 1 (MVP)**: Bundle sqlite-vec extension binary
   - Download pre-compiled `.dylib` for macOS (your platform)
   - Embed in binary using `include_bytes!` macro
   - Extract to temp directory at runtime
   - Use ONNX for embeddings

2. **Phase 2 (Optimization)**: Add Option B as fallback
   - Implement in-memory ANN search
   - Use if sqlite-vec fails to load
   - Better performance for cold start

### Modified Dependencies for Option A

Update `Cargo.toml`:

```toml
[dependencies]
# SQLite with extension support
rusqlite = { version = "0.34", features = ["bundled", "load_extension"] }

# ONNX for embeddings
ort = "2.0"
ort-extras = "2.0"
ort-download = "0.1"  # For model download helper

# Vector operations
ndarray = "0.16"

# Bundle sqlite-vec extension
# Download at build time or include pre-compiled binary
```

### Modified Implementation for Option A

**Update**: `src/memory/store.rs` - add extension loading:

```rust
pub fn new(db_path: &Path) -> Result<Self> {
    let conn = Connection::open(db_path)?;
    
    #[cfg(target_os = "macos")]
    const VSS_LIB: &[u8] = include_bytes!("../../bundles/sqlite-vec.arm64.dylib");
    
    #[cfg(target_os = "macos")]
    {
        use std::env::temp_dir;
        let vss_path = temp_dir().join("total-recall-vss.dylib");
        std::fs::write(&vss_path, VSS_LIB)?;
        conn.load_extension(&vss_path)?;
        conn.execute("SELECT load_vss();", [])?;
    }
    
    // Create tables, enable VSS indexing
}
```

**Update**: `src/memory/embedder.rs` - add model download:

```rust
pub fn new() -> Result<Self> {
    let model_dir = std::env::var("HOME")?
        .parse::<PathBuf>()?
        .join(".total-recall")
        .join("models");
    
    std::fs::create_dir_all(&model_dir)?;
    
    let model_path = model_dir.join("all-MiniLM-L6-v2.onnx");
    
    if !model_path.exists() {
        tracing::info!("Downloading embedding model (5MB)...");
        // Download from HuggingFace
        // Or bundle in binary
    }
    
    // Initialize ONNX session
}
```

### Critical Testing Additions

Add tests to validate Option A:

```rust
// tests/store_tests.rs
#[test]
fn test_sqlite_vec_extension_loads() {
    let (store, _temp) = create_test_db();
    
    // Verify VSS extension loaded successfully
    let result: Result<(), rusqlite::Error> = 
        store.connection.execute("SELECT load_vss();", []);
    
    assert!(result.is_ok());
}

#[test]
fn test_vector_insertion() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## Test
- [note] Vector test"#;
    
    store.create_note("03/13/2026", content).unwrap();
    
    // Check that observations have embeddings
    let notes = store.get_notes().unwrap();
    assert!(notes[0].observations[0].embedding.is_some());
}
```

---

## Decision Required

**Before implementation, choose**:

1. **Option A (Bundle VSS Extension)**: 
   - Requires finding/preparing sqlite-vec binary for macOS
   - Best balance of requirements
   - Add 1-2 days to setup time

2. **Option B (Custom ANN)**:
   - Pure Rust, no binary dependencies
   - Requires implementing similarity search algorithm
   - Add 3-4 days to implementation time

3. **Hybrid (Option A + B fallback)**:
   - Best of both worlds
   - Most robust
   - Requires all of above

**Recommendation**: **Option A with Option B fallback** - implement both, prefer VSS if available, fall back to in-memory ANN if extension fails to load.

---

## Action Items

1. **[PENDING]** Confirm sqlite-vec version compatible with bundled SQLite
2. **[PENDING]** Find/prep sqlite-vec binary for macOS (arm64)
3. **[PENDING]** Decide ONNX model source (download vs bundle)
4. **[REQUIRED]** Add "sqlite-vec extension" setup step to implementation.md
5. **[REQUIRED]** Add "ANN fallback" to implementation.md
6. **[REQUIRED]** Update testing section to validate both paths

**This is blocking for clean implementation - must resolve before coding.**
