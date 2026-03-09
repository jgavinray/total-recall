# Bender TR-7 Checkpoint — Write README

## Task: TR-7 — Write README — no user documentation exists

**Problem Statement:**
`plan.md` references a `README.md` as a deliverable but none exists. Without a README, setup and integration with Claude Desktop or opencode is not documented.

**Acceptance Criteria:**
1. README.md exists at repo root
2. User can follow it from zero to running MCP server
3. Claude Desktop config snippet is accurate
4. Conventional commit with body (Problem/Solution/Notes format)
5. PATCH board to in-review

**Suggested Approach:**
- Read the existing plan.md and source code to understand what the project does
- Write a comprehensive README covering: what it is, prerequisites, build/install, running the server, Claude Desktop config, and basic usage
- No new npm dependencies (this is a Rust project)

## Instructions:
1. First, save these instructions to a checkpoint file: `/Users/jgavinray/dev/total-recall/bender-TR-7-prompt.md`
2. Explore the repo to understand the project: `ls /Users/jgavinray/dev/total-recall/ && cat /Users/jgavinray/dev/total-recall/plan.md`
3. Read relevant source files to understand the MCP server, its tools, and config format
4. Write README.md at repo root
5. Make a git commit with conventional commit format AND a full body (Problem:, Solution:, Notes:)
6. PATCH the board to in-review

**Constraints:**
- No new dependencies
- Conventional commit with full Problem/Solution/Notes body — one-liner commits will be rejected
- Work in: /Users/jgavinray/dev/total-recall/
