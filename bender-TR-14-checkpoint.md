# TR-14 Checkpoint — COMPLETE

**Date:** 2026-03-22  
**Agent:** Bender

## Status: ✅ DONE

## Definition of Done — All Checked

- [x] Container running on hyper01 with proper serve command (no sleep infinity)
- [x] `curl POST http://192.168.0.44:8811/mcp` returns valid MCP InitializeResult
- [x] Committed and pushed (total-recall + ai-homelab repos)
- [x] OpenCode config updated
- [x] TR-14 patched to done

## What Was Done

1. **Root cause of previous failure:** docker-compose command included `/app/total-recall` as a command arg, but Dockerfile has `ENTRYPOINT ["/app/total-recall"]` — so it was double-invoking the binary. Fixed by changing command to just the args: `["serve", "--transport", "http", "--port", "8811", "--host", "0.0.0.0"]`

2. **Also fixed:** Previous image had glibc mismatch (binary built against glibc 2.38, container had older). Fixed by using ubuntu:24.04 for both builder and runtime stages (already done in local Dockerfile, just needed rsync).

3. **Validated:** `curl POST http://192.168.0.44:8811/mcp` returns:
   ```json
   {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"rmcp","version":"1.2.0"},"instructions":"Agentic memory MCP server. Tools: write_note, read_note, search_notes, recent_notes."}}
   ```

4. **Commits:**
   - total-recall: `8de09d9` — feat(TR-14): add Streamable HTTP transport via rmcp, deploy on hyper01
   - ai-homelab: `e2f9bc7` — feat(TR-14): update total-recall OpenCode config to Streamable HTTP

5. **Kanban:** TR-14 → done
