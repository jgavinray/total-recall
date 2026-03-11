# Contributing to Total Recall

Thank you for your interest in contributing to Total Recall! This guide will help you set up the development environment, run tests, and add new MCP tools to the codebase.

## Table of Contents

- [Setup](#setup)
- [Running Tests](#running-tests)
- [Development Workflow](#development-workflow)
- [Adding a New MCP Tool](#adding-a-new-mcp-tool)
- [PR Checklist](#pr-checklist)
- [Troubleshooting](#troubleshooting)

---

## Setup

### Prerequisites

Before you begin, ensure you have the following installed:

- **Rust 1.70+** - The latest stable version
  ```bash
  # Install via rustup (recommended)
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  
  # Verify installation
  rustc --version
  cargo --version
  ```

- **Git** - For version control
  ```bash
  git --version
  ```

### Clone the Repository

```bash
git clone https://github.com/jgavinray/total-recall.git
cd total-recall
```

### Build the Project

```bash
# Build in debug mode (faster compilation)
cargo build

# Build in release mode (optimized performance)
cargo build --release
```

The first build will download the ONNX embedding model (`all-MiniLM-L6-v2`) to your cache directory. This is a one-time operation.

### Verify Installation

```bash
# Run the MCP server
cargo run -- --help
```

---

## Running Tests

### Run All Tests

```bash
# Run all tests (unit + integration)
cargo test --all

# Run tests with verbose output
cargo test --all -- --nocapture

# Run only unit tests
cargo test --lib

# Run only integration tests
cargo test --test integration
```

### Test Structure

Tests are organized as follows:

- **Unit tests** - Located within their respective source files (inline `#[cfg(test)]` modules)
  - Example: `src/memory/store.rs` contains unit tests for the MemoryStore

- **Integration tests** - Located in the `tests/` directory
  - `tests/integration.rs` - End-to-end tests for the write → read → search flow

### Understanding Test Failures

When tests fail, you'll see output like:

```
thread 'test_write_read_search_flow' panicked at tests/integration.rs:45:10:
assertion failed: !recent.is_empty()
```

This indicates:
1. Which test failed (`test_write_read_search_flow`)
2. Where it failed (`tests/integration.rs:45:10`)
3. What assertion failed

Common failure causes:
- ONNX model not downloaded (run `cargo test` once to cache it)
- SQLite database permissions issues
- Missing dependencies

### Adding a New Unit Test

1. **Find the appropriate source file** - Tests live alongside the code they test

2. **Add a `#[cfg(test)]` module** at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        // Arrange
        let input = "test data";
        
        // Act
        let result = process_data(input);
        
        // Assert
        assert_eq!(result, expected_output);
    }
}
```

3. **Follow the test naming convention** - `test_<function>_<scenario>`

4. **Run your new test**:
```bash
cargo test test_something
```

### Adding a New Integration Test

1. **Edit `tests/integration.rs`**

2. **Use the `make_store()` helper** for a fresh test database:
```rust
#[test]
fn test_new_feature() {
    let (_dir, store) = make_store();
    
    // Your test code here
}
```

3. **Follow the Arrange-Act-Assert pattern** with clear comments

4. **Run the integration tests**:
```bash
cargo test --test integration
```

---

## Development Workflow

### Code Quality Tools

#### Clippy (Linter)

```bash
# Run clippy with all suggestions
cargo clippy -- -D warnings

# Run clippy for all targets
cargo clippy --all-targets -- -D warnings
```

Clippy catches:
- Potential bugs
- Style violations
- Performance issues
- Idiom improvements

#### Rustfmt (Formatter)

```bash
# Format all code
cargo fmt

# Check formatting without changing files
cargo fmt -- --check
```

### Build Commands

```bash
# Debug build (default)
cargo build

# Release build (optimized)
cargo build --release

# Clean and rebuild
cargo clean && cargo build

# Build with all features
cargo build --all-features
```

### Running the Server

```bash
# Run with default settings
cargo run

# Run with custom config
cargo run -- --config /path/to/config.yaml

# Run in release mode (faster)
cargo run --release
```

### Debugging

```bash
# Enable debug logging
RUST_LOG=debug cargo run

# Enable trace logging
RUST_LOG=trace cargo run

# Run tests with output
cargo test -- --nocapture
```

---

## Adding a New MCP Tool

Total Recall uses the `rust-mcp-sdk` for Model Context Protocol (MCP) tool definitions. Here's the step-by-step pattern for adding a new tool:

### Step 1: Define the Tool Struct

In `src/mcp/server.rs`, add a new struct with the `#[macros::mcp_tool]` attribute:

```rust
#[macros::mcp_tool(
    name = "my_new_tool",
    description = "Describe what this tool does in one sentence"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct MyNewTool {
    pub param1: String,
    pub param2: i32,
    // Add fields for each input parameter
    // Types must be serde-serializable (String, i32, bool, etc.)
}
```

**Key points:**
- `name` - The tool name (snake_case, unique)
- `description` - Clear, concise description shown to users
- Fields become the tool's input parameters
- Derive `Debug`, `Deserialize`, `Serialize`, and `JsonSchema`

### Step 2: Implement the Handler Logic

Add a new arm in the `handle_call_tool_request` match statement:

```rust
#[async_trait]
impl ServerHandler for MemoryMcpServer {
    // ... existing code ...

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            // ... existing tools ...

            "my_new_tool" => {
                // Parse arguments
                let args_map = params.arguments.unwrap_or_default();
                let args: MyNewTool = serde_json::from_value(
                    serde_json::Value::Object(args_map)
                )
                .ok()
                .ok_or_else(|| CallToolError::unknown_tool("my_new_tool".to_string()))?;

                // Implement tool logic
                let result = self.process_my_tool(&args.param1, args.param2);

                // Return success or error
                match result {
                    Ok(output) => Ok(CallToolResult::text_content(vec![
                        output.into()
                    ])),
                    Err(e) => Ok(CallToolResult::text_content(vec![
                        format!("Error: {}", e).into()
                    ])),
                }
            }

            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }
}
```

### Step 3: Register the Tool

Add your tool to the `handle_list_tools_request` implementation:

```rust
async fn handle_list_tools_request(
    &self,
    _request: Option<PaginatedRequestParams>,
    _runtime: Arc<dyn McpServer>,
) -> std::result::Result<ListToolsResult, RpcError> {
    Ok(ListToolsResult {
        tools: vec![
            WriteNoteTool::tool(),
            ReadNoteTool::tool(),
            SearchNotesTool::tool(),
            RecentNotesTool::tool(),
            MyNewTool::tool(),  // <- Add your tool here
        ],
        meta: None,
        next_cursor: None,
    })
}
```

### Step 4: Add a Test

Add an integration test in `tests/integration.rs`:

```rust
#[test]
fn test_my_new_tool() {
    let (_dir, store) = make_store();
    
    // Arrange: Set up test data
    let input = "test input";
    
    // Act: Call the tool logic
    let result = process_my_tool(input);
    
    // Assert: Verify the output
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), expected_output);
}
```

### Step 5: Verify and Commit

```bash
# Run tests
cargo test --all

# Run clippy
cargo clippy --all-targets -- -D warnings

# Format code
cargo fmt

# Commit with conventional commit format
git add src/mcp/server.rs tests/integration.rs
git commit -m "feat: add my_new_tool MCP tool

Problem: Need ability to [use case].
Solution: Added my_new_tool with [parameters] that [does what].
Notes: Follows the existing tool pattern from [reference tool]."
```

### Complete Example: Adding an "Archive Note" Tool

Here's a concrete example of adding a tool to archive a note:

**1. Define the struct:**
```rust
#[macros::mcp_tool(
    name = "archive_note",
    description = "Archive a note by date"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ArchiveNoteTool {
    pub date: String,
}
```

**2. Implement the handler:**
```rust
"archive_note" => {
    let args_map = params.arguments.unwrap_or_default();
    let args: ArchiveNoteTool = serde_json::from_value(
        serde_json::Value::Object(args_map)
    )
    .ok()
    .ok_or_else(|| CallToolError::unknown_tool("archive_note".to_string()))?;

    let store = self.store.read().await;
    match store.archive_note(&args.date) {
        Ok(_) => Ok(CallToolResult::text_content(vec![
            format!("Archived note for {}", args.date).into()
        ])),
        Err(e) => Ok(CallToolResult::text_content(vec![
            format!("Error: {}", e).into()
        ])),
    }
}
```

**3. Register it:**
```rust
tools: vec![
    // ... existing tools ...
    ArchiveNoteTool::tool(),
]
```

**4. Add a test:**
```rust
#[test]
fn test_archive_note_tool() {
    let (_dir, store) = make_store();
    
    // Create a note first
    store.create_note("2026-03-15", "Test content").unwrap();
    
    // Archive it
    let result = store.archive_note("2026-03-15");
    
    assert!(result.is_ok());
    
    // Verify it's archived
    let note = store.read_note("2026-03-15").unwrap();
    assert!(note.archived);
}
```

---

## PR Checklist

Before submitting a pull request, ensure:

- [ ] **Tests pass** - `cargo test --all` completes successfully
- [ ] **Clippy is clean** - `cargo clippy --all-targets -- -D warnings` has no warnings
- [ ] **Code is formatted** - `cargo fmt` has been run
- [ ] **Commit message has body** - Use the Problem/Solution/Notes format
- [ ] **README updated** - If you changed any public API or tool interface
- [ ] **New tools documented** - Added to this CONTRIBUTING.md if applicable
- [ ] **Integration tests added** - For new MCP tools or major features

### Commit Message Format

```
feat(TR-XX): short description of change

Problem: What was missing or broken.

Solution: What you changed and how.

Notes: Any additional context, references to existing patterns, or follow-up work.
```

**Example:**
```
feat(TR-10): add CONTRIBUTING.md with dev workflow and MCP tool pattern

Problem: No contributing guide. Contributors don't know how to run tests,
what the PR bar is, or how to add a new MCP tool to the codebase.

Solution: Wrote CONTRIBUTING.md covering dev setup, test structure, how to
add a new MCP tool (with concrete example), and PR checklist. Docs live
at repo root for visibility.

Notes: This guide enables self-service contributions. Refer to existing
MCP tools (observe, upsert_notes, search_notes) as patterns.
```

---

## Troubleshooting

### Common Errors

#### Linker Errors

**Error:**
```
error: linking with `cc` failed
```

**Fix:**
```bash
# Install build essentials
# macOS:
xcode-select --install

# Linux:
sudo apt-get install build-essential
```

#### ONNX Model Download Issues

**Error:**
```
Error downloading model: connection timed out
```

**Fix:**
- Check your internet connection
- The model is cached after first download at `~/.cache/total-recall/`
- Manual download: Fetch `all-MiniLM-L6-v2.onnx` from HuggingFace and place in cache dir

#### SQLite Pragma Warnings

**Error:**
```
warning: PRAGMA journal_mode=WAL failed
```

**Fix:**
- This is usually a permissions issue with the database directory
- Ensure you have write permissions to the data directory
- Try deleting the database file and letting it recreate:
  ```bash
  rm -rf ~/.local/share/total-recall/*.db*
  ```

#### Missing sqlite-vec Extension

**Error:**
```
sqlite-vec not loaded
```

**Fix:**
```bash
# Clean and rebuild
cargo clean
cargo build

# Ensure cc crate can compile C code
# (see "Linker Errors" above)
```

#### Test Failures on First Run

**Error:**
```
thread panicked at embedder init should succeed
```

**Fix:**
- The ONNX model downloads on first run
- Wait for the download to complete (may take 1-2 minutes)
- Run tests again: `cargo test --all`

### Getting Help

1. **Check existing issues** - See if your problem has been reported
2. **Read the code** - Comments in `src/mcp/server.rs` and `tests/integration.rs` provide context
3. **Enable debug logging** - `RUST_LOG=debug cargo run` for more detail

---

## Code of Conduct

Be respectful, constructive, and helpful. This is an open project, and we welcome contributions from everyone.

---

## License

By contributing, you agree that your contributions will be licensed under the GPL-2.0 license, the same as the project.
