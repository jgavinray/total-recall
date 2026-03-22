# TR-14 Checkpoint Log

## Step 1: Prompt saved
- Copied spec to bender-TR-14-prompt.md ✅
- Checkpoint file initialized ✅

## Step 2: Codebase understood
- Cargo.toml: rust-mcp-sdk v0.8 with stdio feature
- src/mcp/server.rs: ServerHandler + tool macros from rust-mcp-sdk
- src/main.rs: StdioTransport only
- src/memory/embedder.rs: cache_dir uses dirs::cache_dir() ignoring config ← bug

## Step 3: Research rmcp
- rmcp latest stable: 1.2.0 (not 0.16 as spec suggested)
- Feature flag: `transport-streamable-http-server`
- Uses axum 0.8 + StreamableHttpService + LocalSessionManager
- Example: counter_streamhttp.rs in rust-sdk examples

## Step 4+5: Migration
- Replaced rust-mcp-sdk with rmcp 1.2 in Cargo.toml
- Added axum 0.8, tokio-util, schemars dependencies
- Rewrote src/mcp/server.rs: MemoryMcpServer with #[tool_router] / #[tool_handler] macros
- Tools: write_note, read_note, search_notes, recent_notes

## Step 6: CLI --transport flag added
- `serve --transport stdio` (default) or `serve --transport http --port 8811 --host 0.0.0.0`

## Step 7: ONNX model cache fix
- embedder.rs cache_dir() now checks TR_MODEL_CACHE_DIR env var first
- main.rs sets this from config.embedding.cache_dir before creating embedder

## Step 8: Local build + test ✅
- `cargo build --release` succeeds
- stdio test: InitializeResult returned correctly
- HTTP test: POST /mcp returns SSE event with valid MCP InitializeResult


