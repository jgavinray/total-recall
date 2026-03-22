# Total-Recall Quick Start (5 Minutes)

## Prerequisites

- **OS:** macOS or Linux
- **Rust:** 1.70+ (`rustup install stable` if needed)
- **Tools:** `cargo`, `git`

## Clone + Build

```bash
git clone https://github.com/jgavinray/total-recall.git
cd total-recall
cargo build --release
```

## Configure

```bash
mkdir -p ~/.total-recall && cp config.yaml.example ~/.total-recall/config.yaml
```

Edit `~/.total-recall/config.yaml`:
```yaml
memory_dir: ~/.total-recall
db_path: ~/.total-recall/memory.db
logging:
  level: info
search:
  default_limit: 10
```

## Connect to Claude Desktop

Add to `~/Library/Application Support/Claude/mcp_servers/total-recall.json`:

```json
{
  "mcpServers": {
    "total-recall": {
      "command": "/path/to/total-recall",
      "args": ["serve"],
      "cwd": "~/.total-recall"
    }
  }
}
```

Restart Claude Desktop.

## Verify

In Claude, try:
```
search_notes("test query")
```

Returns empty initially. Create a note, search again.

**That's it.** Ready to build your second brain.
