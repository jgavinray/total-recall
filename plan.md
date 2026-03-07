# Total-Recall: Agentic Memory MCP Server

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                     Agentic System (Claude/VSC)                  │
│                         MCP Client                               │
└────────────────────────────┬────────────────────────────────────┘
                             │ MCP Protocol (stdio)
                             ▼
┌─────────────────────────────────────────────────────────────────┐
│                    total-recall MCP Server                       │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  MCP Handlers                                            │   │
│  │  - write_note()                                          │   │
│  │  - read_note()                                           │   │
│  │  - search_notes()                                        │   │
│  │  - recent_notes()                                        │   │
│  │  - build_context()                                       │   │
│  └──────────────────────────────────────────────────────────┘   │
│                             │                                    │
│                             ▼                                    │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Memory Store                                            │   │
│  │  - Note CRUD operations                                  │   │
│  │  - Observation parsing                                   │   │
│  │  - Vector embedding (ONNX all-MiniLM-L6-v2, 384-dim)    │   │
│  └──────────────────────────────────────────────────────────┘   │
│                             │                                    │
│                    ┌────────┴────────┐                          │
│                    ▼                 ▼                          │
│          ┌─────────────────┐  ┌─────────────────┐               │
│          │  /~/.total-     │  │  ~/.total-      │               │
│          │  recall/        │  │  recall/        │               │
│          │  03/06/2026.md  │  │  memory.db      │               │
│          │  03/05/2026.md  │  │  + vss index    │               │
│          └─────────────────┘  └─────────────────┘               │
└─────────────────────────────────────────────────────────────────┘
```

## File Structure
```
total-recall/
├── Cargo.toml                  # Dependencies
├── plan.md                     # This file
├── README.md                   # User documentation
├── build.rs                    # Build script for sqlite-vec
├── config.yaml.example         # Configuration template
├── src/
│   ├── main.rs                 # CLI entry, MCP serve
│   ├── config.rs               # Config file loading (YAML)
│   ├── memory/
│   │   ├── mod.rs              # Memory module exports
│   │   ├── store.rs            # SQLite + ONNX operations
│   │   ├── models.rs           # Note, Observation types
│   │   ├── embedder.rs         # ONNX embedding model
│   │   └── file_parser.rs      # Markdown parsing, observation extraction
│   ├── mcp/
│   │   ├── mod.rs              # MCP module exports
│   │   ├── server.rs           # MCP server implementation
│   │   └── tools/
│   │       ├── write_note.rs   # write_note tool
│   │       ├── read_note.rs    # read_note tool
│   │       ├── search_notes.rs # search_notes tool
│   │       ├── recent_notes.rs # recent_notes tool
│   │       └── build_context.rs # build_context tool
│   └── error.rs                # Error types
└── examples/
    └── demo.md                   # Example usage
```

## Memory File Organization

```
~/.total-recall/
├── 03-2026/                    # Month-year subdirectory
│   ├── 06-03-2026.md
│   ├── 05-03-2026.md
│   ├── 04-03-2026.md
│   └── 03-03-2026.md
├── 02-2026/
│   ├── 28-02-2026.md
│   └── 27-02-2026.md
└── memory.db                   # SQLite index
```

**Naming convention**: `mm-dd-yyyy.md` (month first, dash-separated)

**Directory structure**: `mm-yyyy/` subdirectories for better organization

**Complete example**:
```
~/.total-recall/
├── 03-2026/                    # mm-yyyy/ subdirectory
│   ├── 03-06-2026.md           # mm-dd-yyyy.md filename (March 6th)
│   ├── 03-05-2026.md           # March 5th
│   └── 03-04-2026.md           # March 4th
├── 02-2026/
│   └── 02-28-2026.md           # February 28th
└── memory.db
```

## MCP Tools Specification

### write_note
Creates a new dated markdown file (immutably). Errors if file exists.

```json
{
  "name": "write_note",
  "description": "Create a new memory note. Returns error if date already exists.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "content": {
        "type": "string",
        "description": "The note content (markdown format)"
      },
      "timestamp": {
        "type": "string",
        "description": "Optional timestamp section header (e.g., '14:30')",
        "default": null
      }
    },
    "required": ["content"]
  }
}
```

**Immutability:** Files cannot be overwritten. To add content to existing dates, use `append_note` instead.
```

### read_note
Reads a note by date path or permalink.

```json
{
  "name": "read_note",
  "description": "Read a note by date or permalink",
  "inputSchema": {
    "type": "object",
    "properties": {
      "date": {
        "type": "string",
        "description": "Date in mm/dd/yyyy format"
      },
      "permalink": {
        "type": "string",
        "description": "Optional permalink identifier"
      }
    }
  }
}
```

### search_notes
Full-text and vector semantic search across all notes.

```json
{
  "name": "search_notes",
  "description": "Search notes using full-text or semantic vector search",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Search query - can be keyword or semantic"
      },
      "limit": {
        "type": "integer",
        "description": "Maximum results to return",
        "default": 10
      },
      "search_type": {
        "type": "string",
        "enum": ["fulltext", "vector"],
        "description": "Type of search to perform",
        "default": "vector"
      },
      "days": {
        "type": "integer",
        "description": "Only search notes from last N days",
        "default": null
      }
    },
    "required": ["query"]
  }
}
```

### recent_notes
Get notes from the last N days (archived excluded by default).

```json
{
  "name": "recent_notes",
  "description": "Get notes from the last N days, ordered by update time",
  "inputSchema": {
    "type": "object",
    "properties": {
      "limit": {
        "type": "integer",
        "description": "Maximum number of notes to return",
        "default": 10
      },
      "days": {
        "type": "integer",
        "description": "Number of days to look back",
        "default": 7
      },
      "include_archived": {
        "type": "boolean",
        "description": "Include archived/deleted notes",
        "default": false
      }
    }
  }
}
```

### build_context
Build semantic context from observations, not graph traversal.

```json
{
  "name": "build_context",
  "description": "Retrieve observations from a date with semantic relationships",
  "inputSchema": {
    "type": "object",
    "properties": {
      "date": {
        "type": "string",
        "description": "Date in mm/dd/yyyy format"
      },
      "include_details": {
        "type": "boolean",
        "description": "Return full observation context",
        "default": true
      },
      "category_filter": {
        "type": "string",
        "description": "Optional: filter by category (decision, action, etc.)"
      }
    },
    "required": ["date"]
  }
}
```

**Note:** This returns all observations from a date. Real "graph traversal" across dates can be approximated by searching for tags/categories that appear in multiple notes.
```

## Immutability & Write Protection

**Files are IMMUTABLE once written** to prevent accidental corruption:

| Operation | Behavior | Safeguard |
|-----------|----------|-----------|
| `write_note` on existing date | ✗ Returns error: "File exists" | Prevents overwrite |
| `append_note` | ✓ Creates new entry with timestamp | Append-only, preserves original |
| `delete_note` | ⚠ Soft delete (marks as archived) | Original retained, just hidden |
| `edit_note` | ✗ Not supported - use `append` | Never modifies existing content |

**Key principle:** Once memory is written, it cannot be deleted or modified.
- Users can query `recent_notes` with `include_archived=false` to hide old entries
- Original files preserved for historical integrity
- Aligns with how human memory works - we add, we don't erase

### File Naming
```
mm/dd/yyyy.md
```

Examples:
- `03/06/2026.md` - Current date
- `01/15/2026.md` - Meeting from last month

### Append-Only Content Format

Entries include timestamped sections:

```markdown
---
title: "2026-03-06"
date: 2026-03-06
type: entry
archived: false
---

## 14:30
Initial discussion about project direction

## 14:45
- [decision] Adopt Rust for microservice #architecture
- [action] John to evaluate ONNX runtime

## 15:30
Follow-up on action items

```

### Markdown Content Structure

```markdown
---
title: "2026-03-06 Team Sync"
date: 2026-03-06
type: meeting
tags:
  - team
  - sync
---

## Discussion Points

- [decision] Adopting Rust for new microservice #architecture
- [action] John to evaluate ONNX runtime for embeddings
- [note] Need to set up monitoring before launch

## Action Items

- [ ] Set up GitHub Actions CI
- [ ] Deploy to staging environment
```

### Observation Parsing

The system will parse lines matching the pattern `- [category] content #tag`:

| Category | Description |
|----------|-------------|
| `decision` | Final decisions made |
| `action` | Action items with assignees |
| `note` | General observations |
| `idea` | Potential improvements |
| `question` | Open questions |
| `risk` | Identified risks |

## Database Schema

```sql
-- Notes table (one row per mm/dd/yyyy.md file)
CREATE TABLE notes (
    id TEXT PRIMARY KEY,
    date TEXT NOT NULL UNIQUE,          -- mm/dd/yyyy
    title TEXT NOT NULL,
    full_content TEXT NOT NULL,         -- raw markdown content
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    content_hash TEXT NOT NULL,         -- SHA256 of content (detect changes)
    archived INTEGER DEFAULT 0,         -- soft delete flag
    content_version INTEGER DEFAULT 1   -- version counter
);

-- Observations table (vector chunks - one row per semantic unit)
CREATE TABLE observations (
    id TEXT PRIMARY KEY,
    note_id TEXT NOT NULL,              -- foreign key to notes
    timestamp TEXT NOT NULL,            -- when in file (e.g., "14:30")
    section TEXT,                       -- section header (e.g., "## Discussion")
    category TEXT,                      -- decision, action, note, idea
    content TEXT NOT NULL,              -- the actual chunk text
    full_context TEXT,                  -- surrounding markdown context
    tags TEXT,                          -- JSON array of tags
    embedding FLOAT[384],               -- vector embedding (384-dim from MiniLM)
    version INTEGER DEFAULT 1           -- for future append tracking
);

-- Indexes
CREATE INDEX idx_observations_note_id ON observations(note_id);
CREATE INDEX idx_observations_category ON observations(category);
CREATE INDEX idx_observations_embedding ON observations USING vss(embedding);
CREATE INDEX idx_notes_date ON notes(date DESC);
CREATE INDEX idx_notes_archived ON notes(archived);
```

## Chunking Strategy

**Observation-based chunking** (not by note or fixed size):

- Each observation line = one vector chunk
- Natural semantic unit (decision, action, note, idea, question, risk)
- Better search granularity and embedding accuracy
- Can search by category, tags, or free-text

Example chunk:
```
note_id: 03/06/2026
timestamp: "14:45"
category: "decision"
content: "Adopting Rust for new microservice"
tags: ["architecture", "rust"]
embedding: [0.123, -0.456, ...]  // 384-dim vector
```

-- Indexes
CREATE INDEX idx_observations_embedding ON observations USING vss(embedding);
CREATE INDEX idx_notes_date ON notes(date DESC);
CREATE INDEX idx_notes_content ON notes(full_content);
```

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1)
- [ ] Set up project structure and dependencies
- [ ] Implement models (Note, Observation) with immutability
- [ ] SQLite database setup with sqlite-vss
- [ ] File I/O for mm/dd/yyyy.md (append-only, no overwrites)
- [ ] Error handling with clear messages for "file exists" errors
- [ ] Soft delete/archival mechanism

### Phase 2: Embedding System (Week 1-2)
- [ ] ONNX runtime integration (ort crate)
- [ ] Download/init all-MiniLM-L6-v2 model
- [ ] Text embedding pipeline
- [ ] Vector storage to SQLite

### Phase 3: MCP Server (Week 2)
- [ ] MCP server implementation (mcp crate)
- [ ] write_note tool
- [ ] read_note tool
- [ ] search_notes tool (vector + fulltext)
- [ ] recent_notes tool
- [ ] build_context tool

### Phase 4: Polish & Testing (Week 2-3)
- [ ] Unit tests for all modules
- [ ] Integration tests with MCP client
- [ ] Documentation (README, usage examples)
- [ ] Claude Desktop configuration example
- [ ] Performance optimization

### Phase 5: Additional Features (Optional)
- [ ] File watching for real-time sync
- [ ] Relationship extraction (auto-generate links)
- [ ] Tag-based filtering
- [ ] Export/import functionality

## Key Dependencies

| Crate | Purpose | Notes |
|-------|---------|-------|
| `mcp` | MCP protocol | Model Context Protocol SDK |
| `rusqlite` + `sqlite-vss` | SQLite + vectors | Vector similarity search |
| `ort` + `ort-extras` | ONNX runtime | Model inference for embeddings |
| `chrono` | Date handling | mm/dd/yyyy parsing/formatting |
| `tracing` | Logging | Structured logging |
| `thiserror` | Error handling | User-friendly errors |
| `serde` | Serialization | JSON/metadata handling |

## Configuration

User configuration via `config.yaml` file or CLI overriding defaults:

**Default location**: `~/.total-recall/config.yaml` (can be changed via `--config` flag)

```yaml
# config.yaml
memory_dir: ~/.total-recall
db_path: ~/.total-recall/memory.db
log_level: info
embedding_model: sentence-transformers/all-MiniLM-L6-v2
onnx_model_dir: ~/.total-recall/models

logging:
  file: ~/.total-recall/logs/server.log
  max_size_mb: 10
  backup_count: 3

search:
  default_limit: 10
  max_limit: 100

# Optional: Custom paths
# memory_dir: /path/to/my/notes
# db_path: /path/to/notes.db
```

**CLI overrides** (if needed):
```bash
total-recall serve --config /custom/path/config.yaml
total-recall rebuild-index --config /custom/path/config.yaml
```

## Claude Desktop Integration

Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "total-recall": {
      "command": "total-recall",
      "args": [
        "--memory-dir",
        "~/.total-recall"
      ],
      "env": {}
    }
  }
}
```

## Success Criteria

1. **Functional**: All 5 MCP tools work correctly
2. **Performance**: Search < 100ms for 1000 notes
3. **Compatibility**: Works with Claude Desktop and opencode
4. **Reliability**: No data loss, proper error handling
5. **Usability**: Clear documentation, sensible defaults
