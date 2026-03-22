# Bender TR-12/TR-13 Checkpoint
**Started:** 2026-03-21 20:53 PDT

---

## Step 1: Initial Assessment
**Status:** COMPLETE

### Findings:
- **local binary:** `target/debug/total-recall` exists (release binary does NOT exist - never built for release)
- **SSH to hyper01:** BLOCKED — mac-mini key (`jgavinray@zoidberg.local`) not in hyper01's `authorized_keys`. The `authorized_hosts` file on mac has `jgavinray@edgecase` key which is a different machine.
- **Plugin:** exists at `~/dev/ai-homelab/zoidberg/openclaw/extensions/total-recall-tools/` (not installed to `~/.openclaw/extensions/`)
- **openclaw.json subagents.tools.alsoAllow:** only has `openrag_search` — TR tools missing
- **Critical bug found:** `write` CLI command enforces 1-note-per-day immutability. `tr_store` calls `write` which exits with code 1 after first call per day. **tr_store is broken for repeat calls.**

### Actions needed:
1. Fix immutability: add `--append` flag to `write` or add `append` subcommand
2. Build release binary
3. Fix SSH (need Gavin to add mac-mini key to hyper01 OR use password auth)
4. Add TR tools to subagents.alsoAllow
5. Deploy docker on hyper01 (blocked on SSH)
6. Wire OpenCode MCP config (TR-13)

---

## Step 2: Fix write immutability + build release binary
**Status:** IN PROGRESS

