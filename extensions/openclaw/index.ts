/**
 * Total Recall Tools Plugin
 *
 * Exposes tr_store, tr_search, and tr_recall as interactive tools for episodic
 * memory access via the total-recall MCP HTTP server (Streamable HTTP, MCP spec 2025-06-18).
 *
 * Tool mapping:
 *   tr_store  → MCP tools/call write_note
 *   tr_search → MCP tools/call search_notes
 *   tr_recall → MCP tools/call recent_notes
 *
 * Configuration:
 *   config.plugins.entries["total-recall-tools"].config.serverUrl
 *     (default: http://localhost:8811/mcp)
 *
 * Implementation notes:
 *   - Uses native fetch (Node 18+) — available in the OpenClaw plugin runtime
 *   - Each tool call establishes a fresh MCP session (initialize → initialized → tools/call)
 *   - Server returns SSE-formatted responses; we parse the JSON data line
 */

const ANSI_STRIP = /\x1B\[[0-9;]*[mGKHF]/g;

function stripAnsi(str: string): string {
  return str.replace(ANSI_STRIP, "").trim();
}

/** Parse a text/event-stream body and return the last JSON data payload found. */
function parseSseData(body: string): any {
  const lines = body.split("\n");
  let last: any = null;
  for (const line of lines) {
    if (line.startsWith("data: ") && line.length > 6) {
      try {
        last = JSON.parse(line.slice(6));
      } catch {
        // skip non-JSON data lines (empty pings etc.)
      }
    }
  }
  return last;
}

interface McpSession {
  sessionId: string;
  serverUrl: string;
  timeoutMs: number;
}

async function initMcpSession(serverUrl: string, timeoutMs: number): Promise<McpSession> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  let sessionId: string;
  try {
    const res = await fetch(serverUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method: "initialize",
        params: {
          protocolVersion: "2024-11-05",
          capabilities: {},
          clientInfo: { name: "openclaw-total-recall-tools", version: "0.2.0" },
        },
        id: 1,
      }),
      signal: controller.signal,
    });

    if (!res.ok) {
      throw new Error(`MCP initialize failed: HTTP ${res.status}`);
    }

    const raw = await res.text();
    const data = parseSseData(raw);
    if (!data?.result) {
      throw new Error(`MCP initialize returned unexpected response: ${raw.slice(0, 200)}`);
    }

    sessionId = res.headers.get("mcp-session-id") ?? "";
    if (!sessionId) {
      throw new Error("MCP server did not return a session ID");
    }
  } finally {
    clearTimeout(timer);
  }

  // Send initialized notification (no response expected, ignore result)
  fetch(serverUrl, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json, text/event-stream",
      "mcp-session-id": sessionId,
    },
    body: JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized", params: {} }),
  }).catch(() => {});

  return { sessionId, serverUrl, timeoutMs };
}

async function mcpToolCall(
  session: McpSession,
  toolName: string,
  args: Record<string, unknown>
): Promise<string> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), session.timeoutMs);

  try {
    const res = await fetch(session.serverUrl, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
        "mcp-session-id": session.sessionId,
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        method: "tools/call",
        params: { name: toolName, arguments: args },
        id: 10,
      }),
      signal: controller.signal,
    });

    if (!res.ok) {
      throw new Error(`MCP tools/call failed: HTTP ${res.status}`);
    }

    const raw = await res.text();
    const data = parseSseData(raw);

    if (!data) {
      throw new Error(`No JSON data in MCP response: ${raw.slice(0, 200)}`);
    }

    if (data.error) {
      throw new Error(`MCP error ${data.error.code}: ${data.error.message}`);
    }

    if (data.result?.isError) {
      const errContent = data.result.content
        ?.map((c: any) => c.text ?? "")
        .join(" ")
        .trim();
      throw new Error(`Tool error: ${errContent || "unknown tool error"}`);
    }

    const text = (data.result?.content ?? [])
      .filter((c: any) => c.type === "text")
      .map((c: any) => c.text ?? "")
      .join("\n")
      .trim();

    return stripAnsi(text);
  } finally {
    clearTimeout(timer);
  }
}

export default function register(api: any) {
  const logger = api.logger;
  const cfg = api.config?.plugins?.entries?.["total-recall-tools"]?.config ?? {};

  const serverUrl: string = cfg.serverUrl ?? "http://localhost:8811/mcp";
  const timeoutMs: number = cfg.timeoutMs ?? 15000;

  // ─── tr_store ─────────────────────────────────────────────────────────────
  api.registerTool({
    name: "tr_store",
    label: "Total Recall: Store Memory",
    description:
      "Store a new episodic memory note in total-recall. " +
      "Use this to persist important context, decisions, findings, or summaries " +
      "that should be retrievable in future sessions.",
    parameters: {
      type: "object",
      properties: {
        content: {
          type: "string",
          description: "The memory content to store. Be specific and include relevant context.",
        },
      },
      required: ["content"],
    },
    async execute(_toolCallId: string, params: any) {
      const { content } = params;
      if (!content || typeof content !== "string" || !content.trim()) {
        return {
          content: [{ type: "text", text: "Error: content is required and must be non-empty." }],
          details: { success: false, error: "empty content" },
        };
      }

      try {
        const session = await initMcpSession(serverUrl, timeoutMs);
        const output = await mcpToolCall(session, "write_note", { content: content.trim() });
        const text = output || "Memory stored successfully.";
        logger.debug(`tr_store: ${text}`);
        return {
          content: [{ type: "text", text }],
          details: { success: true, output: text },
        };
      } catch (err: any) {
        const msg = err.message || "Unknown error";
        logger.warn(`tr_store error: ${msg}`);
        return {
          content: [{ type: "text", text: `Error storing memory: ${msg}` }],
          details: { success: false, error: msg },
        };
      }
    },
  });

  // ─── tr_search ────────────────────────────────────────────────────────────
  api.registerTool({
    name: "tr_search",
    label: "Total Recall: Search Memories",
    description:
      "Semantically search episodic memories stored in total-recall. " +
      "Returns relevant memory notes matching the query. " +
      "Use this to find prior context, decisions, or knowledge from past sessions.",
    parameters: {
      type: "object",
      properties: {
        query: {
          type: "string",
          description: "Search query — be specific for better results.",
        },
        limit: {
          type: "number",
          description: "Maximum number of results to return (default: 10).",
          minimum: 1,
          maximum: 50,
        },
        include_archived: {
          type: "boolean",
          description: "Include archived notes in search results (default: false).",
        },
      },
      required: ["query"],
    },
    async execute(_toolCallId: string, params: any) {
      const { query, limit, include_archived } = params;
      if (!query || typeof query !== "string" || !query.trim()) {
        return {
          content: [{ type: "text", text: "Error: query is required and must be non-empty." }],
          details: { success: false, error: "empty query" },
        };
      }

      const args: Record<string, unknown> = { query: query.trim() };
      if (limit) args.limit = Math.min(50, Math.max(1, limit));
      if (include_archived != null) args.include_archived = include_archived;

      try {
        const session = await initMcpSession(serverUrl, timeoutMs);
        const output = await mcpToolCall(session, "search_notes", args);
        const text = output || "No matching memories found.";
        return {
          content: [{ type: "text", text }],
          details: { success: true, output: text },
        };
      } catch (err: any) {
        const msg = err.message || "Unknown error";
        logger.warn(`tr_search error: ${msg}`);
        return {
          content: [{ type: "text", text: `Error searching memories: ${msg}` }],
          details: { success: false, error: msg },
        };
      }
    },
  });

  // ─── tr_recall ────────────────────────────────────────────────────────────
  api.registerTool({
    name: "tr_recall",
    label: "Total Recall: Recall Recent Memories",
    description:
      "Retrieve recent episodic memory notes from total-recall. " +
      "Returns a summary of notes from the past N days. " +
      "Use this to reconstruct recent context or catch up after a session gap.",
    parameters: {
      type: "object",
      properties: {
        days: {
          type: "number",
          description: "How many days back to retrieve notes from (default: 7).",
          minimum: 1,
          maximum: 365,
        },
        limit: {
          type: "number",
          description: "Maximum number of notes to return (default: 10).",
          minimum: 1,
          maximum: 50,
        },
        include_archived: {
          type: "boolean",
          description: "Include archived notes (default: false).",
        },
      },
      required: [],
    },
    async execute(_toolCallId: string, params: any) {
      const { days, limit, include_archived } = params ?? {};

      const args: Record<string, unknown> = {};
      if (days) args.days = Math.min(365, Math.max(1, days));
      if (limit) args.limit = Math.min(50, Math.max(1, limit));
      if (include_archived != null) args.include_archived = include_archived;

      try {
        const session = await initMcpSession(serverUrl, timeoutMs);
        const output = await mcpToolCall(session, "recent_notes", args);
        const text = output || "No recent memories found.";
        return {
          content: [{ type: "text", text }],
          details: { success: true, output: text },
        };
      } catch (err: any) {
        const msg = err.message || "Unknown error";
        logger.warn(`tr_recall error: ${msg}`);
        return {
          content: [{ type: "text", text: `Error recalling memories: ${msg}` }],
          details: { success: false, error: msg },
        };
      }
    },
  });

  logger.info(`Total Recall tools registered: tr_store, tr_search, tr_recall (server: ${serverUrl})`);
}
