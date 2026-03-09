# Task: TR-1 + TR-2 + TR-3 — Fix all 29 Rust compilation errors

**Repo:** `/Users/jgavinray/dev/total-recall`
**Board IDs:** TR-1 (primary), TR-2, TR-3 (all blocking, fix together)

## THE PROBLEM

The code in `src/mcp/server.rs` was written against a wrong/hallucinated rust-mcp-sdk API. The actual installed version is `rust-mcp-sdk 0.8.3`. The API is completely different.

### ERROR GROUP 1: Wrong rust-mcp-sdk API (TR-1 — the big one)

The current code uses:
```rust
use rust_mcp_sdk::server_runtime::server_runtime::ServerRuntime; // WRONG - private
use rust_mcp_sdk::server_runtime::stdio::StdioTransport; // WRONG path
use rust_mcp_sdk::schema::ToolResult; // WRONG - doesn't exist
// impl ServerHandler { fn name(), fn version(), fn description(), fn list_tools() } // WRONG methods
// Tool { ..Default::default() } // WRONG - Tool doesn't impl Default
```

The CORRECT API (from the actual SDK example at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rust-mcp-sdk-0.8.3/examples/quick-start-server-stdio.rs`):

```rust
use rust_mcp_sdk::{
    error::SdkResult,
    mcp_server::{server_runtime, McpServerOptions, ServerHandler},
    schema::*,
    *,
};

// ServerHandler trait uses:
async fn handle_list_tools_request(
    &self,
    _request: Option<PaginatedRequestParams>,
    _runtime: std::sync::Arc<dyn McpServer>,
) -> std::result::Result<ListToolsResult, RpcError>;

async fn handle_call_tool_request(
    &self,
    params: CallToolRequestParams,
    _runtime: std::sync::Arc<dyn McpServer>,
) -> std::result::Result<CallToolResult, CallToolError>;

// Tool construction — NO Default::default():
let tool = Tool {
    name: "tool_name".to_string(),
    description: Some("description".to_string()),
    input_schema: ToolInputSchema { ... },
    annotations: None,
    title: None,
};

// Server startup:
let transport = StdioTransport::new(TransportOptions::default())?;
let handler = MyHandler::default().to_mcp_server_handler();
let server = server_runtime::create_server(McpServerOptions {
    transport,
    handler,
    server_details: InitializeResult { ... },
    task_store: None,
    client_task_store: None,
});
server.start().await
```

### ERROR GROUP 2: SQLite Send+Sync (TR-2)

`Arc<Connection>` fails because `rusqlite::Connection` is not `Send+Sync`. Fix: wrap in `Arc<Mutex<Connection>>` using `std::sync::Mutex`.

Current:
```rust
pub struct MemoryStore {
    connection: Arc<Connection>,
}
```

Fix:
```rust
use std::sync::Mutex;
pub struct MemoryStore {
    connection: Arc<Mutex<Connection>>,
}
```

Update all `self.connection.xxx()` callsites to `self.connection.lock().unwrap().xxx()`.

### ERROR GROUP 3: Type mismatches in store.rs (TR-3)

Common issues:
- `limit as i64` where `String` expected in query params — use `limit.to_string()`
- `Some(title.clone())` where it wraps an `Option<String>` — handle the Option properly
- `load_extension` not found — remove or guard behind a feature flag

### ERROR GROUP 4: main.rs usage of old server API

After fixing server.rs, fix any errors in main.rs too.

## WORKFLOW
1. Save full task prompt (done)
2. Run `cargo build 2>&1` and capture ALL errors
3. Fix imports in `src/mcp/server.rs` — new SDK API
4. Rewrite `impl ServerHandler for MemoryMcpServer` — correct method signatures
5. Fix tool construction (remove `..Default::default()`)
6. Fix `src/memory/store.rs` — wrap Connection in Mutex, fix type errors, remove/guard load_extension
7. Fix `src/main.rs` if needed
8. Run `cargo build 2>&1` after each major change to see progress
9. When `cargo build` succeeds: commit with body
10. PATCH board to in-review when done

## COMMIT FORMAT
Subject: `fix(TR-1/TR-2/TR-3): fix all 29 rust compilation errors`
Body must include Problem:, Solution:, Notes: sections.
