# TR-14 Resume — Hyper01 Deploy Only

The rmcp migration and HTTP transport are DONE. Binary builds and HTTP transport validated locally. 

## Only remaining work: deploy to hyper01

### What's working
- rmcp 1.2.0 migration complete
- `serve --transport http --port 8811 --host 0.0.0.0` works locally
- POST /mcp returns valid MCP InitializeResult

### Blocker from previous run
Dockerfile glibc mismatch. Fix: use `ubuntu:24.04` for BOTH builder and runtime stages (matches hyper01's glibc 2.39). This worked for the previous TR-11 deployment — use `Dockerfile.hyper01`.

### Steps

1. Check/update `Dockerfile.hyper01` — must use ubuntu:24.04 for runtime:
   ```bash
   cat /Users/jgavinray/dev/total-recall/Dockerfile.hyper01
   ```
   If runtime stage is not ubuntu:24.04, fix it.

2. Update `docker-compose.hyper01.yml` — replace `sleep infinity` with proper command:
   ```yaml
   command: ["/app/total-recall", "serve", "--transport", "http", "--port", "8811", "--host", "0.0.0.0"]
   ports:
     - "8811:8811"
   ```

3. Rsync and build on hyper01:
   ```bash
   rsync -avz /Users/jgavinray/dev/total-recall/ zoidberg@192.168.0.44:/home/zoidberg/total-recall/ --exclude target --exclude .git
   ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker build -f Dockerfile.hyper01 -t total-recall:latest . 2>&1 | tail -30'
   ```

4. Deploy:
   ```bash
   ssh zoidberg@192.168.0.44 'cd /home/zoidberg/total-recall && docker compose -f docker-compose.hyper01.yml up -d --force-recreate'
   ```

5. Validate:
   ```bash
   sleep 5
   curl -s -X POST http://192.168.0.44:8811/mcp \
     -H 'Content-Type: application/json' \
     -H 'Accept: application/json, text/event-stream' \
     -H 'MCP-Protocol-Version: 2025-06-18' \
     -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'
   ```
   Must return valid MCP InitializeResult.

6. Commit all changes:
   ```bash
   cd /Users/jgavinray/dev/total-recall
   git add -A
   git commit -m "feat(TR-14): add Streamable HTTP transport via rmcp, deploy on hyper01

Problem: total-recall used stdio-only transport. hyper01 deployment used
sleep infinity + docker exec hack.

Solution: Migrated to rmcp 1.2.0 (official MCP Rust SDK). Added
--transport http flag with Streamable HTTP per spec 2025-06-18.
Fixed ONNX cache to respect TR_MODEL_CACHE_DIR. Updated Dockerfile.hyper01
to ubuntu:24.04. docker-compose.hyper01.yml now uses proper serve command
on port 8811.

Notes: stdio transport retained for backward compat."
   git push
   ```

7. Update OpenCode config:
   ```bash
   cat > ~/.opencode/config.json << 'EOF'
   {
     "mcp": {
       "total-recall": {
         "type": "http",
         "url": "http://192.168.0.44:8811/mcp"
       }
     }
   }
   EOF
   cp ~/.opencode/config.json ~/dev/ai-homelab/zoidberg/opencode/config.json
   cd ~/dev/ai-homelab && git add -A && git commit -m "feat(TR-14): update total-recall OpenCode config to Streamable HTTP" && git push
   ```

8. PATCH kanban:
   ```bash
   curl -X PATCH http://localhost:3000/api/kanban/TR-14 \
     -H 'Content-Type: application/json' \
     -d '{"status":"done","comment":"Streamable HTTP via rmcp deployed on hyper01:8811. OpenCode config updated."}'
   ```

## Definition of Done
- [ ] Container running on hyper01 with proper serve command (no sleep infinity)
- [ ] `curl POST http://192.168.0.44:8811/mcp` returns valid MCP InitializeResult
- [ ] Committed and pushed (total-recall + ai-homelab repos)
- [ ] OpenCode config updated
- [ ] TR-14 patched to done

Append status to: `/Users/jgavinray/dev/total-recall/bender-TR-14-checkpoint.md`
