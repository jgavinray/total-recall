# Bender: TR-14 — Add Streamable HTTP Transport to total-recall

**Date:** 2026-03-22  
**Priority:** P1  
**Repo:** `/Users/jgavinray/dev/total-recall/`  
**Hyper01 SSH:** `zoidberg@192.168.0.44` (ALWAYS use this user)
**Kanban PATCH:** `curl -X PATCH http://localhost:3000/api/kanban/TR-14 -H 'Content-Type: application/json' -d '{"status":"...","comment":"..."}'`

---

## Background

total-recall currently uses `rust-mcp-sdk` (third-party, v0.8) with stdio only. This needs to be migrated to `rmcp` (the **official** Anthropic/MCP Rust SDK from `modelcontextprotocol/rust-sdk`) and given a proper **Streamable HTTP transport** per MCP spec 2025-06-18.

stdio is for local dev tooling. A networked MCP server MUST use Streamable HTTP. The current hyper01 deployment uses `sleep infinity` + `docker exec` which is unacceptable.

---

## Spec

- **Transport spec:** https://modelcontextprotocol.io/specification/2025-06-18/basic/transports.md
- **Official Rust SDK:** https://github.com/modelcontextprotocol/rust-sdk (`rmcp` v0.16.0 on crates.io)
- **Do NOT implement** the old HTTP+SSE transport (2024-11-05) — it's deprecated
- **Do NOT use** mcp-proxy or any other wrapper

### Streamable HTTP Requirements
- Single endpoint `/mcp` supporting both POST and GET
- `POST /mcp` — JSON-RPC from client; server responds with `application/json` or `text/event-stream`
- `GET /mcp` — SSE stream for server→client messages
- Session management via `Mcp-Session-Id` header
- `MCP-Protocol-Version: 2025-06-18` header support
- Origin header validation (security requirement)
- Bind to `0.0.0.0` for container networking

---

## Steps

### 1. Save this prompt
```bash
cp /Users/jgavinray/dev/total-recall/bender-TR-14-http-transport.md /Users/jgavinray/dev/total-recall/bender-TR-14-prompt.md
```

### 2. Understand the current codebase
```bash
cat /Users/jgavinray/dev/total-recall/Cargo.toml
ls /Users/jgavinray/dev/total-recall/src/
cat /Users/jgavinray/dev/total-recall/src/main.rs
cat /Users/jgavinray/dev/total-recall/src/mcp/server.rs 2>/dev/null || ls /Users/jgavinray/dev/total-recall/src/mcp/
```

### 3. Research rmcp HTTP transport
Check `rmcp` docs and features:
```bash
cargo search rmcp 2>/dev/null || true
curl -s https://crates.io/api/v1/crates/rmcp | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['crate']['max_stable_version'])"
curl -s https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/crates/rmcp/Cargo.toml | head -60
```
Look for the HTTP transport feature flag — likely `transport-sse-server`, `transport-streamable-http`, or similar. Check the examples directory:
```bash
curl -s https://api.github.com/repos/modelcontextprotocol/rust-sdk/contents/examples/servers | python3 -c "import sys,json; [print(f['name']) for f in json.load(sys.stdin)]"
```

### 4. Migrate Cargo.toml
Replace `rust-mcp-sdk` with `rmcp`. Add Axum for HTTP if rmcp doesn't bundle it. Example:
```toml
rmcp = { version = "0.16", features = ["server", "<http-transport-feature>"] }
# Only add axum if rmcp doesn't bundle HTTP transport:
# axum = "0.7"
```
Keep all existing dependencies (rusqlite, ort, tokenizers, etc.).

### 5. Migrate MCP server code
The existing MCP server in `src/mcp/` implements tools (`write_note`, `read_note`, `search_notes`, `recent_notes`, `build_context`). Migrate the handler to implement `rmcp::ServerHandler` trait.

Reference the counter example from the SDK:
```bash
curl -s https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/examples/servers/src/common/counter.rs
```

### 6. Add `--transport` CLI flag
In `src/main.rs`, add to the `serve` subcommand:
```rust
#[arg(long, default_value = "stdio")]
transport: String,  // "stdio" or "http"

#[arg(long, default_value = "8811")]
port: u16,

#[arg(long, default_value = "127.0.0.1")]
host: String,
```

When `--transport http`:
- Start the Streamable HTTP server using rmcp's transport
- Bind to `host:port`

When `--transport stdio` (default):
- Use rmcp's stdio transport (keep existing behavior)

### 7. Fix ONNX model cache
The model cache is currently going to `/root/.cache` inside the container, ignoring the config's `cache_dir`. Find where the ONNX model is downloaded in `src/` and ensure it respects the config path. This MUST be fixed — `/archive/zoidberg/total-recall/models` must be used.

### 8. Build and test locally
```bash
cd /Users/jgavinray/dev/total-recall
cargo build --release 2>&1 | tail -20

# Test stdio still works:
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}' | ./target/release/total-recall serve --transport stdio

# Test HTTP:
./target/release/total-recall serve --transport http --port 8811 --host 127.0.0.1 &
sleep 2
curl -s -X POST http://127.0.0.1:8811/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2025-06-18' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'
kill %1
```

### 9. Update docker-compose.hyper01.yml
```yaml
services:
  total-recall:
    image: total-recall:latest
    container_name: total-recall
    restart: unless-stopped
    command: ["/app/total-recall", "serve", "--transport", "http", "--port", "8811", "--host", "0.0.0.0"]
    ports:
      - "8811:8811"
    volumes:
      - /archive/zoidberg/total-recall/memory:/data/memory
      - /archive/zoidberg/total-recall/models:/data/models
    environment:
      - TR_MEMORY_DIR=/data/memory
      - TR_MODEL_CACHE_DIR=/data/models
```

### 10. Rebuild and redeploy on hyper01
```bash
# Sync repo
rsync -avz /Users/jgavinray/dev/total-recall/ zoidberg@192.168.0.44:/home/zoidberg/total-recall/ --exclude target --exclude .git

# Build on hyper01
ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker build -f Dockerfile.hyper01 -t total-recall:latest . 2>&1 | tail -20'

# Redeploy
ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker compose -f docker-compose.hyper01.yml up -d --force-recreate'

# Validate
sleep 5
ssh zoidberg@192.168.0.44 'docker ps | grep total-recall'
curl -s -X POST http://192.168.0.44:8811/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2025-06-18' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'
```

### 11. Update OpenCode MCP config
Update `~/.opencode/config.json` AND `~/dev/ai-homelab/zoidberg/opencode/config.json`:
```json
{
  "mcp": {
    "total-recall": {
      "type": "http",
      "url": "http://192.168.0.44:8811/mcp"
    }
  }
}
```

### 12. Commit everything
```bash
cd /Users/jgavinray/dev/total-recall
git add -A
git commit -m "feat(TR-14): add Streamable HTTP transport via rmcp

Problem: total-recall used stdio-only transport (acceptable for local
dev tooling, not for networked MCP servers). hyper01 deployment was using
sleep infinity + docker exec as a workaround.

Solution: Migrated from rust-mcp-sdk to rmcp (official MCP Rust SDK,
modelcontextprotocol/rust-sdk). Added --transport http flag implementing
Streamable HTTP per MCP spec 2025-06-18. Single /mcp endpoint with POST
and GET, session management via Mcp-Session-Id, Origin header validation.
Fixed ONNX model cache to respect config cache_dir. Updated
docker-compose.hyper01.yml to use proper serve command on port 8811.

Notes: stdio transport retained for backward compat. OpenCode config
updated to use http transport at http://192.168.0.44:8811/mcp."

cd ~/dev/ai-homelab
git add -A
git commit -m "feat(TR-14): update total-recall OpenCode config to use Streamable HTTP

Updated zoidberg/opencode/config.json to point at http://192.168.0.44:8811/mcp
instead of ssh+docker exec. Requires total-recall serving HTTP on hyper01."
git push
```

### 13. PATCH kanban
```bash
curl -X PATCH http://localhost:3000/api/kanban/TR-14 \
  -H 'Content-Type: application/json' \
  -d '{"status":"done","comment":"Streamable HTTP transport implemented via rmcp. total-recall serving on hyper01:8811/mcp. OpenCode config updated. Committed to ai-homelab + total-recall repos."}'
```

---

## Checkpointing
Append status after each major step to: `/Users/jgavinray/dev/total-recall/bender-TR-14-checkpoint.md`

## If You Get Stuck
- If rmcp doesn't have a built-in Streamable HTTP transport feature: implement it manually using Axum following the spec. The spec is clear — it's just HTTP POST + GET on a single endpoint with SSE streaming.
- If the rmcp migration is too complex (API very different from rust-mcp-sdk): consider keeping rust-mcp-sdk for the tool logic and adding a separate Axum HTTP layer that speaks Streamable HTTP and delegates tool calls to the existing handler. But try rmcp first.
- After 3 failed attempts on any single step: document the blocker clearly and report back.

## Definition of Done
- [ ] `rmcp` is the MCP dependency (not rust-mcp-sdk)
- [ ] `total-recall serve --transport http --port 8811` starts a Streamable HTTP server
- [ ] POST to `http://192.168.0.44:8811/mcp` with InitializeRequest returns valid MCP response
- [ ] Container on hyper01 uses proper serve command (no `sleep infinity`)
- [ ] ONNX model cache goes to `/archive/zoidberg/total-recall/models`
- [ ] OpenCode config uses HTTP transport
- [ ] Committed to both repos
- [ ] TR-14 patched to done
