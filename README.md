# Total-Recall

**Agentic memory MCP server** — persistent, searchable notes with semantic vector search, backed by SQLite and plain Markdown files.

Total-Recall gives AI agents (Claude Desktop, opencode, or any MCP-compatible client) a long-term memory layer. Notes are written as human-readable Markdown files and indexed in SQLite with 384-dimensional vector embeddings for semantic search.

---

## What It Does

- **Stores notes** as dated Markdown files (`mm-dd-yyyy.md`) under `~/.total-recall/`
- **Indexes observations** (decisions, actions, notes, ideas, questions, risks) into SQLite
- **Embeds content** using `all-MiniLM-L6-v2` (via ONNX) for semantic similarity search
- **Exposes 5 MCP tools** to any connected AI agent:
  - `write_note` — create a new dated note (immutable once written)
  - `read_note` — retrieve a note by date
  - `search_notes` — full-text or vector semantic search
  - `recent_notes` — list notes from the last N days
  - `build_context` — retrieve structured observations from a date

Notes are **immutable by design** — once written they cannot be overwritten, mirroring how human memory works.

---

## Prerequisites

- **Rust** ≥ 1.85 (2024 edition) — [install via rustup](https://rustup.rs/)
- **Git**
- macOS or Linux (Windows untested)
- **Internet access** on first run — the ONNX embedding model (~90 MB) is downloaded automatically to `~/.total-recall/models/`

---

## Build & Install

```bash
# 1. Clone the repo
git clone https://github.com/jgavinray/total-recall.git
cd total-recall

# 2. Build in release mode
cargo build --release

# 3. Install the binary to your PATH
cargo install --path .
```

After install, verify it works:

```bash
total-recall --help
```

You should see the subcommand list (`serve`, `write`, `read`, `search`, `recent`).

---

## Configuration

Copy the example config and customize as needed:

```bash
mkdir -p ~/.total-recall
cp config.yaml.example ~/.total-recall/config.yaml
```

Default `~/.total-recall/config.yaml`:

```yaml
memory_dir: ~/.total-recall
db_path: ~/.total-recall/memory.db

logging:
  level: info
  file: ~/.total-recall/logs/server.log
  max_size_mb: 10
  backup_count: 3

embedding:
  model: sentence-transformers/all-MiniLM-L6-v2
  dimension: 384
  cache_dir: ~/.total-recall/models

search:
  default_limit: 10
  max_limit: 100
  similarity_threshold: 0.7

mcp:
  enabled: true
  stdio: true
  timeout_seconds: 30
```

A custom config path can be specified with `--config /path/to/config.yaml`.

---

## Running the MCP Server

Start the server manually to verify it works before wiring it to a client:

```bash
total-recall serve
```

The server communicates over **stdio** using the MCP protocol. You should see log output like:

```
2026-03-09T... INFO total_recall: Loading Total-Recall from "~/.total-recall/config.yaml"
2026-03-09T... INFO total_recall: Memory directory: "~/.total-recall"
2026-03-09T... INFO total_recall: Starting MCP server via stdio...
2026-03-09T... INFO total_recall: Memory store initialized at "~/.total-recall/memory.db"
```

> **Note:** On first run, the embedding model is downloaded automatically. This may take a moment depending on your connection.

---

## Claude Desktop Integration

Add total-recall to your Claude Desktop MCP server config.

**Config location:**
- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Linux: `~/.config/Claude/claude_desktop_config.json`

**Edit the file** (create it if it doesn't exist):

```json
{
  "mcpServers": {
    "total-recall": {
      "command": "total-recall",
      "args": ["serve"],
      "env": {}
    }
  }
}
```

If `total-recall` is not on your shell `$PATH` (e.g. Claude Desktop launches in a restricted environment), use the full binary path:

```json
{
  "mcpServers": {
    "total-recall": {
      "command": "/Users/yourname/.cargo/bin/total-recall",
      "args": ["serve"],
      "env": {}
    }
  }
}
```

Find your binary path with:

```bash
which total-recall
# or
echo ~/.cargo/bin/total-recall
```

**Restart Claude Desktop** after editing the config. You'll see total-recall listed under MCP tools in the sidebar.

---

## opencode Integration

Add to your opencode MCP configuration (`.opencode.json` or equivalent):

```json
{
  "mcp": {
    "servers": {
      "total-recall": {
        "command": "total-recall",
        "args": ["serve"]
      }
    }
  }
}
```

---

## CLI Usage

Total-Recall also works as a standalone CLI without an MCP client:

```bash
# Write a new note for today
total-recall write "Decided to use Rust for the memory layer. #architecture"

# Write with an explicit timestamp section
total-recall write "Meeting notes: discussed Q2 roadmap" --timestamp 14:30

# Read a note by date (mm/dd/yyyy format)
total-recall read 03/09/2026

# Search notes semantically (vector search by default)
total-recall search "architecture decisions"

# Search with a result limit
total-recall search "deployment" --limit 5

# Get recent notes (last 7 days by default)
total-recall recent

# Recent notes from the last 30 days
total-recall recent --days 30 --limit 20
```

---

## MCP Tools Reference

Once connected to an MCP client, agents can call these tools:

### `write_note`
Create a new dated note. Errors if a note for the current date already exists (immutability).

| Parameter   | Type   | Required | Description                            |
|-------------|--------|----------|----------------------------------------|
| `content`   | string | ✅        | Note content in Markdown format        |
| `timestamp` | string | ❌        | Optional section header (e.g. `14:30`) |

### `read_note`
Retrieve a note by date.

| Parameter   | Type   | Required | Description                    |
|-------------|--------|----------|--------------------------------|
| `date`      | string | ✅        | Date in `mm/dd/yyyy` format    |

### `search_notes`
Search across all notes using full-text or vector semantic search.

| Parameter     | Type    | Default    | Description                              |
|---------------|---------|------------|------------------------------------------|
| `query`       | string  | (required) | Keyword or semantic search query         |
| `limit`       | integer | `10`       | Maximum number of results                |
| `search_type` | string  | `"vector"` | `"vector"` or `"fulltext"`              |
| `days`        | integer | null       | Only search notes from last N days       |

### `recent_notes`
List notes from the last N days, ordered by update time.

| Parameter         | Type    | Default | Description                         |
|-------------------|---------|---------|-------------------------------------|
| `limit`           | integer | `10`    | Maximum number of notes to return   |
| `days`            | integer | `7`     | Number of days to look back         |
| `include_archived`| boolean | `false` | Include archived/soft-deleted notes |

### `build_context`
Retrieve structured observations from a date with optional category filtering.

| Parameter         | Type    | Default | Description                                       |
|-------------------|---------|---------|---------------------------------------------------|
| `date`            | string  | (required) | Date in `mm/dd/yyyy` format                    |
| `include_details` | boolean | `true`  | Return full observation context                   |
| `category_filter` | string  | null    | Filter by category: `decision`, `action`, `note`, `idea`, `question`, `risk` |

---

## Note Format

Notes are stored as Markdown with YAML frontmatter:

```markdown
---
title: "2026-03-09"
date: 2026-03-09
type: entry
archived: false
---

## 14:30
Initial kickoff discussion.

## 15:00
- [decision] Adopt Rust for the memory layer #architecture
- [action] Download and test ONNX runtime integration
- [note] Vector search performs well under 100ms for 1000 notes
```

**Observation categories** (used in `- [category] content #tag` lines):

| Category   | Description                 |
|------------|-----------------------------|
| `decision` | Final decisions made        |
| `action`   | Action items with assignees |
| `note`     | General observations        |
| `idea`     | Potential improvements      |
| `question` | Open questions              |
| `risk`     | Identified risks            |

---

## Data Storage

```
~/.total-recall/
├── 03-2026/               # mm-yyyy/ month directory
│   ├── 03-09-2026.md      # mm-dd-yyyy.md daily note
│   ├── 03-08-2026.md
│   └── 03-07-2026.md
├── 02-2026/
│   └── 02-28-2026.md
├── models/                # Downloaded ONNX model (auto)
│   └── all-MiniLM-L6-v2/
├── logs/
│   └── server.log
├── memory.db              # SQLite index + vector embeddings
└── config.yaml            # Your config
```

---

## Troubleshooting

**Model download fails on first run**
Ensure you have internet access. The ONNX model (~90 MB) downloads to `~/.total-recall/models/` automatically. If it fails, retry by restarting the server.

**Claude Desktop doesn't show total-recall tools**
1. Verify the binary path with `which total-recall`
2. Use the full absolute path in the config (not `~`)
3. Check Claude Desktop logs for MCP connection errors
4. Restart Claude Desktop after config changes

**"Note for date already exists" error**
Notes are immutable once written per date. This is by design. Use `search_notes` or `read_note` to access existing content.

**Database errors on startup**
Remove and recreate the database (note: this loses the index, not the Markdown files):
```bash
rm ~/.total-recall/memory.db
total-recall serve  # rebuilds index on next write
```

---

## License

GPL-2.0 — see [LICENSE](LICENSE).
