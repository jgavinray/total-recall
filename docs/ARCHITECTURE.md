# Architecture — total-recall

> A semantic memory MCP server: write notes, search them by meaning.

---

## 1. Component Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                      MCP Client (AI Agent)                  │
│       (Claude, any MCP-compatible host via stdio)           │
└──────────────────────────┬──────────────────────────────────┘
                           │ JSON-RPC / stdio (MCP protocol)
┌──────────────────────────▼──────────────────────────────────┐
│                      MCP Layer                               │
│   src/mcp/server.rs — MemoryMcpServer                        │
│   Tools: write_note │ read_note │ search_notes │ recent_notes │
└──────┬────────────────────────────────────┬─────────────────┘
       │ store ops                           │ embed queries
┌──────▼──────────────────┐   ┌─────────────▼───────────────┐
│     Memory Layer        │   │     Embedding Layer         │
│  src/memory/store.rs    │   │  src/memory/embedder.rs     │
│  MemoryStore            │   │  Embedder (ONNX session)    │
│  create/read/search     │   │  all-MiniLM-L6-v2 (384d)   │
└──────┬──────────────────┘   └─────────────────────────────┘
       │
┌──────▼──────────────────────────────────────────────────────┐
│                   SQLite Database                            │
│   notes          — one row per daily note                    │
│   observations   — parsed bullet-point observations          │
│   vec_observations — sqlite-vec virtual table (KNN search)  │
└─────────────────────────────────────────────────────────────┘
```

**Primary transports:** The server communicates over stdio (default). The `main.rs` CLI also provides direct subcommands (`write`, `read`, `search`, `recent`) for command-line use.

---

## 2. Module Breakdown

| File | Responsibility |
|------|---------------|
| `src/main.rs` | Binary entry point: parses CLI args, loads config, dispatches to MCP server mode or direct subcommands (`write`, `read`, `search`, `recent`). |
| `src/lib.rs` | Re-exports `config`, `error`, and `memory` modules so the `tests/` integration suite can import them. |
| `src/config.rs` | YAML config struct (`Config`) with defaults for `memory_dir`, `db_path`, embedding, search limits, logging, and MCP server settings. Loaded from `~/.total-recall/config.yaml`. |
| `src/error.rs` | `MemoryError` enum (thiserror) covering database, I/O, parse, embedding, date, and MCP error variants; `Result<T>` type alias. |
| `src/mcp/mod.rs` | Module declaration for the MCP layer — re-exports `server`. |
| `src/mcp/server.rs` | `MemoryMcpServer` struct implementing `rust-mcp-sdk`'s `ServerHandler` trait; defines the four MCP tool structs and dispatches `handle_call_tool_request` to the memory store. |
| `src/memory/mod.rs` | Module declaration for the memory layer — re-exports `models`, `file_parser`, `store`, `embedder`. |
| `src/memory/models.rs` | Domain types: `Note`, `NoteMetadata` (with YAML frontmatter parser), `Observation`. |
| `src/memory/file_parser.rs` | `FileParser::parse_observations` — walks note text line by line to extract timestamped bullet observations with categories and hashtag tags. |
| `src/memory/store.rs` | `MemoryStore` — SQLite wrapper (rusqlite + sqlite-vec); owns all database operations: `create_note`, `read_note`, `search_notes`, `get_recent_notes`, `archive_note`, `restore_note`. |
| `src/memory/embedder.rs` | `Embedder` — downloads and caches the all-MiniLM-L6-v2 ONNX model and tokenizer, tokenizes text, runs ONNX inference, mean-pools with attention mask, L2-normalizes → 384-dim vector. |

---

## 3. Data Model

### SQLite Tables

**`notes`** — one row per daily note:

```sql
CREATE TABLE notes (
    id          TEXT PRIMARY KEY,   -- UUID v4
    date        TEXT NOT NULL UNIQUE, -- "mm-dd-yyyy" (enforced unique)
    title       TEXT,               -- from YAML frontmatter or date string
    content     TEXT NOT NULL,      -- raw markdown content
    created_at  INTEGER NOT NULL,   -- Unix timestamp
    updated_at  INTEGER NOT NULL,   -- Unix timestamp
    archived    INTEGER DEFAULT 0   -- 0 = active, 1 = archived
);
```

**`observations`** — parsed bullet-point items extracted from note content:

```sql
CREATE TABLE observations (
    id        TEXT NOT NULL UNIQUE, -- UUID v4
    note_id   TEXT NOT NULL,        -- references notes.date
    timestamp TEXT NOT NULL,        -- "HH:MM" header under which obs appeared
    section   TEXT,                 -- "## Section" heading above the timestamp
    category  TEXT,                 -- bracket tag, e.g. "task", "note", "idea"
    content   TEXT NOT NULL,        -- cleaned observation text
    context   TEXT NOT NULL,        -- original raw line preserved verbatim
    tags      TEXT                  -- JSON array of hashtag strings
);
```

**`vec_observations`** — sqlite-vec virtual table (vec0) for KNN search:

```sql
CREATE VIRTUAL TABLE vec_observations USING vec0(
    embedding float[384]            -- rowid matches observations.rowid
);
```

Indexes: `idx_observations_note_id`, `idx_observations_category`, `idx_notes_date`, `idx_notes_archived`.

### Note File Format

Notes are stored as raw markdown in the `content` column. The parser expects this structure:

```markdown
---
title: "Daily Note"
date: "03-09-2026"
type: daily
tags:
  - work
archived: false
---

## Work

## 10:30
- [task] Shipped the embedder refactor #rust #embeddings
- [note] Reviewed PR for main branch

## 14:00
- [idea] Add batch embed support for faster indexing
```

- YAML frontmatter between `---` delimiters is parsed by `NoteMetadata::parse_frontmatter`.
- `## HH:MM` headers set the current timestamp context for subsequent observations.
- `## Section` headers (non-timestamp) set the section context.
- Lines matching `- [category] text #tag` are extracted as `Observation` rows.

---

## 4. Embedding Pipeline

All semantic indexing and search goes through `src/memory/embedder.rs`:

1. **Model acquisition** — On first use, `Embedder::new()` downloads `all-MiniLM-L6-v2.onnx` and `all-MiniLM-L6-v2-tokenizer.json` from HuggingFace into `~/.cache/total-recall/`. Subsequent starts use the cache.

2. **Tokenization** — `tokenizers` crate encodes input text into `input_ids`, `attention_mask`, and `token_type_ids` tensors. Truncates at 128 tokens; right-pads batches to longest sequence.

3. **ONNX inference** — `ort` (ONNX Runtime) runs the BERT encoder. Output `[0]` is `last_hidden_state` with shape `[batch, seq_len, 384]`.

4. **Mean pooling** — Token embeddings are summed weighted by the attention mask, then divided by the mask sum to produce a single 384-dim vector per input.

5. **L2 normalization** — The pooled vector is divided by its L2 norm, producing a unit vector suitable for cosine similarity via dot product.

6. **Storage** — `MemoryStore.parse_and_insert_observations` calls `embedder.embed(obs.content)` for each observation, serializes the vector to a JSON array string, and inserts via `INSERT INTO vec_observations(rowid, embedding) VALUES (?, vec_f32(?))`.

7. **KNN search** — `store.search_notes` issues a CTE query using sqlite-vec's `MATCH vec_f32(?)` operator with `LIMIT` pushed inside the `WITH knn AS (...)` subquery. Results are joined back to `observations` → `notes` ordered by ascending distance.

---

## 5. MCP Tool → Function Mapping

| MCP Tool | Struct | Handler path | Store function |
|----------|--------|-------------|----------------|
| `write_note` | `WriteNoteTool` | `handle_call_tool_request` → `"write_note"` branch | `MemoryStore::create_note` |
| `read_note` | `ReadNoteTool` | `handle_call_tool_request` → `"read_note"` branch | `MemoryStore::read_note` |
| `search_notes` | `SearchNotesTool` | `handle_call_tool_request` → `"search_notes"` branch | `Embedder::embed` → `MemoryStore::search_notes` |
| `recent_notes` | `RecentNotesTool` | `handle_call_tool_request` → `"recent_notes"` branch | `MemoryStore::get_recent_notes` |

All tools are registered in `handle_list_tools_request` using the `#[mcp_tool]` macro from `rust-mcp-sdk`. The `MemoryMcpServer` holds an `Arc<RwLock<MemoryStore>>` and `Arc<Embedder>` for shared access across async handler invocations.

---

## 6. Data Flow — Note Creation to Search

```
write_note("Shipped the embedder refactor #rust")
    │
    ▼
MemoryMcpServer::handle_call_tool_request("write_note")
    │  get current date → "03-11-2026"
    ▼
MemoryStore::create_note(date, content)
    │  INSERT INTO notes (...)
    │
    ├─► FileParser::parse_observations(content)
    │       parse ## HH:MM, ## Section, - [category] lines
    │       → Vec<Observation>
    │
    └─► for each Observation:
            INSERT INTO observations (...)
            Embedder::embed(obs.content) → [f32; 384]
            INSERT INTO vec_observations(rowid, vec_f32(...))

── later: search_notes("embedder refactor") ──

    ▼
Embedder::embed("embedder refactor") → query_vec: [f32; 384]
    │
    ▼
MemoryStore::search_notes(query_vec, limit=10)
    │  WITH knn AS (
    │      SELECT rowid, distance FROM vec_observations
    │      WHERE embedding MATCH vec_f32(query_vec) LIMIT 10
    │  )
    │  SELECT n.* FROM knn JOIN observations o JOIN notes n
    │  ORDER BY distance ASC
    │
    ▼
Vec<Note> returned → formatted as text content in CallToolResult
```

---

## Key Dependencies

| Crate | Version | Role |
|-------|---------|------|
| `rust-mcp-sdk` | 0.8 | MCP protocol (stdio transport, tool macros, server runtime) |
| `rusqlite` | 0.30 | SQLite bindings (bundled SQLite) |
| `sqlite-vec` | 0.1.6 | sqlite-vec extension (vec0 virtual table, KNN search) |
| `ort` | 2.0.0-rc.12 | ONNX Runtime for embedding inference |
| `tokenizers` | 0.21 | HuggingFace tokenizer (all-MiniLM-L6-v2, fancy-regex backend) |
| `tokio` | 1.46 | Async runtime |
| `clap` | 4.5 | CLI argument parsing |
| `serde_yaml` | 0.9 | Config file parsing |
