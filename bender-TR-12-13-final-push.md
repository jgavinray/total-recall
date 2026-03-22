# Bender: Total-Recall Final Push — TR-12 + TR-13
**Date:** 2026-03-21  
**Priority:** P0 — must ship tonight  
**Assignee:** Bender  
**Reviewer:** Professor  

---

## Context

Total-recall is a Rust MCP memory server that stores episodic notes as Markdown files indexed in SQLite with vector search. The binary is at `/Users/jgavinray/dev/total-recall/target/release/total-recall`. The repo is at `/Users/jgavinray/dev/total-recall/`.

### What's Done
- TR-11: Dockerfile + docker-compose exist in repo. Binary builds.
- TR-12 partial: Plugin built (commit 4653847 in ai-homelab), tools `tr_store`, `tr_search`, `tr_recall` registered. **NOT YET end-to-end tested.**

### What Needs to Happen Tonight

---

## TASK 1 — Deploy total-recall on hyper01 as Docker container

**Goal:** total-recall running persistently on hyper01 (192.168.0.44), data persisted to `/archive/zoidberg/total-recall` on the host.

### Steps

1. **Check if Docker is running on hyper01:**
   ```bash
   ssh jgavinray@192.168.0.44 'docker ps'
   ```

2. **Ensure /archive/zoidberg/total-recall exists:**
   ```bash
   ssh jgavinray@192.168.0.44 'sudo mkdir -p /archive/zoidberg/total-recall/memory /archive/zoidberg/total-recall/models && sudo chown -R jgavinray:jgavinray /archive/zoidberg/total-recall'
   ```

3. **Copy the repo to hyper01 (or build locally and push image):**
   Option A — rsync repo and build on hyper01:
   ```bash
   rsync -avz /Users/jgavinray/dev/total-recall/ jgavinray@192.168.0.44:/home/jgavinray/total-recall/ --exclude target --exclude .git
   ```
   Then on hyper01:
   ```bash
   ssh jgavinray@192.168.0.44 'cd /home/jgavinray/total-recall && docker build -t total-recall:latest .'
   ```

4. **Create a production docker-compose for hyper01** at `/home/jgavinray/total-recall/docker-compose.hyper01.yml`:

   The compose file must:
   - Mount `/archive/zoidberg/total-recall/memory` as memory dir
   - Mount `/archive/zoidberg/total-recall/models` as model cache
   - Set `restart: unless-stopped`
   - Name the container `total-recall`
   - Expose an MCP proxy on port 8811 via `mcp-proxy` (see below) OR use `npx @modelcontextprotocol/inspector` — check what's available

   **IMPORTANT:** total-recall speaks MCP over stdio only. To make it network-accessible for other services, it needs to be wrapped. Use `mcp-proxy`:
   ```bash
   # Check if mcp-proxy available on hyper01
   ssh jgavinray@192.168.0.44 'which mcp-proxy || pip install mcp-proxy'
   ```
   
   If mcp-proxy is available, the compose should run:
   ```
   mcp-proxy --port 8811 -- total-recall serve
   ```
   and expose port 8811.

   If mcp-proxy is NOT available: run total-recall as a long-running service (it will idle) and access it via `docker exec` / CLI subcommands. The OpenClaw plugin uses subprocess anyway.

5. **Start it:**
   ```bash
   ssh jgavinray@192.168.0.44 'cd /home/jgavinray/total-recall && docker compose -f docker-compose.hyper01.yml up -d'
   ```

6. **Validate it's running:**
   ```bash
   ssh jgavinray@192.168.0.44 'docker ps | grep total-recall'
   ssh jgavinray@192.168.0.44 'docker exec total-recall total-recall write "TR-12 deployment test — Bender was here" 2>&1'
   ssh jgavinray@192.168.0.44 'docker exec total-recall total-recall recent 2>&1'
   ssh jgavinray@192.168.0.44 'ls /archive/zoidberg/total-recall/memory/'
   ```
   The last command must show a dated `.md` file.

---

## TASK 2 — TR-12: End-to-end test of OpenClaw plugin

The plugin already exists (commit 4653847). The remaining acceptance criterion is:
> **Test: store a memory from main session, retrieve it from a subagent session**

BUT — there's an issue. The current plugin uses subprocess invocation of the local `total-recall` binary. Now that we're deploying on hyper01, we need to decide:
- Does the plugin call `total-recall` on the mac-mini (synced to hyper01)?
- Or does it SSH into hyper01 and call the container?

**Decision:** The plugin should call the **local** total-recall binary (which is already on the mac-mini at `/Users/jgavinray/dev/total-recall/target/release/total-recall`) with a config that points its data dir to a path that is **synced** with hyper01 — OR, simpler, update the plugin to SSH to hyper01 and use `docker exec total-recall total-recall <subcommand>`.

Check the current plugin code:
```bash
ls /Users/jgavinray/.openclaw/extensions/total-recall-tools/
cat /Users/jgavinray/.openclaw/extensions/total-recall-tools/index.js 2>/dev/null || cat /Users/jgavinray/.openclaw/extensions/total-recall-tools/index.ts 2>/dev/null
```

Also check ai-homelab:
```bash
ls ~/dev/ai-homelab/zoidberg/openclaw/extensions/total-recall-tools/ 2>/dev/null
```

Evaluate the plugin code and determine if it needs to be updated to point to hyper01. Then:
1. If it needs updating: update, test, commit
2. Run end-to-end test: call `tr_store` from main context, then verify retrieval

---

## TASK 3 — TR-13: Wire OpenCode/ACP MCP config

Once total-recall is serving on hyper01 (port 8811 if mcp-proxy works, otherwise skip to CLI approach):

1. Find Bender's OpenCode config:
   ```bash
   ls ~/dev/ai-homelab/zoidberg/ | grep -i opencode
   cat ~/.opencode/config.json 2>/dev/null || cat ~/.config/opencode/config.json 2>/dev/null
   ```

2. Add total-recall MCP server entry pointing to `mcp-proxy` on hyper01:
   ```json
   {
     "mcp": {
       "servers": {
         "total-recall": {
           "command": "ssh",
           "args": ["jgavinray@192.168.0.44", "mcp-proxy-client http://localhost:8811"]
         }
       }
     }
   }
   ```
   OR if using stdio directly:
   ```json
   {
     "mcp": {
       "servers": {
         "total-recall": {
           "command": "ssh",
           "args": ["jgavinray@192.168.0.44", "docker exec -i total-recall total-recall serve"]
         }
       }
     }
   }
   ```

3. Commit to ai-homelab.

---

## TASK 4 — Validate tools available to all LLMs

After deployment, validate:
1. OpenClaw main agent (Zoidberg): call `tr_store` and `tr_search` — confirm tools work
2. Confirm the `tools.subagents.alsoAllow` in openclaw.json includes `tr_store`, `tr_search`, `tr_recall` so subagents can access them
3. Check openclaw.json:
   ```bash
   cat ~/.openclaw/openclaw.json | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('tools',{}).get('subagents',{}))"
   ```

---

## Checkpointing Rules (MANDATORY)
1. Save this prompt to: `/Users/jgavinray/dev/total-recall/bender-TR-12-13-final-push.md` ✅ (already done)
2. After each major step, append status to: `/Users/jgavinray/dev/total-recall/bender-TR-12-13-checkpoint.md`
3. PATCH kanban tickets as you complete each task:
   - TR-12 → `done` when plugin e2e tested
   - TR-13 → `done` when OpenCode config committed

Kanban PATCH endpoint: `curl -X PATCH http://localhost:3000/api/kanban/<id> -H 'Content-Type: application/json' -d '{"status":"done","comment":"..."}'`

---

## Definition of Done

- [ ] total-recall Docker container running on hyper01, data at `/archive/zoidberg/total-recall`
- [ ] `docker exec total-recall total-recall recent` returns results
- [ ] `/archive/zoidberg/total-recall/memory/` contains dated `.md` files  
- [ ] OpenClaw plugin `tr_store` / `tr_search` / `tr_recall` work end-to-end
- [ ] OpenCode MCP config committed to ai-homelab
- [ ] `tools.subagents.alsoAllow` includes TR tools
- [ ] TR-12 and TR-13 patched to `done` on kanban

**Do not declare done until ALL items are checked.**
