use crate::error::Result;
use crate::memory::embedder::Embedder;
use crate::memory::store::MemoryStore;
use async_trait::async_trait;
use rust_mcp_sdk::{
    macros,
    mcp_server::{server_runtime, McpServerOptions, ServerHandler},
    schema::*,
    *,
};
use std::sync::Arc;
use tokio::sync::RwLock;

// Define tools using the macro
#[macros::mcp_tool(
    name = "write_note",
    description = "Create a new memory note"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct WriteNoteTool {
    pub content: String,
}

#[macros::mcp_tool(
    name = "read_note",
    description = "Read a note by date in mm-dd-yyyy format"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReadNoteTool {
    pub date: String,
}

#[macros::mcp_tool(
    name = "search_notes",
    description = "Search notes semantically"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct SearchNotesTool {
    pub query: String,
}

#[macros::mcp_tool(
    name = "recent_notes",
    description = "Get recent notes"
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RecentNotesTool {
    pub days: i32,
}

pub struct MemoryMcpServer {
    store: Arc<RwLock<MemoryStore>>,
    embedder: Arc<Embedder>,
}

impl MemoryMcpServer {
    pub fn new(store: MemoryStore, _memory_dir: std::path::PathBuf) -> Result<Self> {
        let embedder = Embedder::new()?;
        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            embedder: Arc::new(embedder),
        })
    }
}

#[async_trait]
impl ServerHandler for MemoryMcpServer {
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
            ],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            "write_note" => {
                let args_map = params.arguments.unwrap_or_default();
                let args: WriteNoteTool = serde_json::from_value(serde_json::Value::Object(args_map))
                    .ok()
                    .ok_or_else(|| CallToolError::unknown_tool("write_note".to_string()))?;

                let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
                let store = self.store.read().await;
                match store.append_note(&current_date, &args.content) {
                    Ok(_) => Ok(CallToolResult::text_content(vec![
                        format!("Stored memory for {}", current_date).into(),
                    ])),
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        Ok(CallToolResult::text_content(vec![error_msg.into()]))
                    }
                }
            }
            "read_note" => {
                let args_map = params.arguments.unwrap_or_default();
                let args: ReadNoteTool = serde_json::from_value(serde_json::Value::Object(args_map))
                    .ok()
                    .ok_or_else(|| CallToolError::unknown_tool("read_note".to_string()))?;

                let store = self.store.read().await;
                match store.read_note(&args.date) {
                    Ok(note) => {
                        let content = format!("## {}\n\n{}", note.date, note.content);
                        Ok(CallToolResult::text_content(vec![content.into()]))
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        Ok(CallToolResult::text_content(vec![error_msg.into()]))
                    }
                }
            }
            "search_notes" => {
                let args_map = params.arguments.unwrap_or_default();
                let args: SearchNotesTool = serde_json::from_value(serde_json::Value::Object(args_map))
                    .ok()
                    .ok_or_else(|| CallToolError::unknown_tool("search_notes".to_string()))?;

                let query_embedding = self.embedder.embed(&args.query);
                let store = self.store.read().await;

                match store.search_notes(&query_embedding, 10, false) {
                    Ok(notes) => {
                        let content = notes
                            .iter()
                            .map(|n| format!("- {}", n.date))
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(CallToolResult::text_content(vec![content.into()]))
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        Ok(CallToolResult::text_content(vec![error_msg.into()]))
                    }
                }
            }
            "recent_notes" => {
                let args_map = params.arguments.unwrap_or_default();
                let args: RecentNotesTool = serde_json::from_value(serde_json::Value::Object(args_map))
                    .ok()
                    .ok_or_else(|| CallToolError::unknown_tool("recent_notes".to_string()))?;

                let days = if args.days > 0 { args.days as usize } else { 7 };
                let store = self.store.read().await;
                match store.get_recent_notes(10, days, false) {
                    Ok(notes) => {
                        let content = notes
                            .iter()
                            .map(|n| format!("- {}", n.date))
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(CallToolResult::text_content(vec![content.into()]))
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        Ok(CallToolResult::text_content(vec![error_msg.into()]))
                    }
                }
            }
            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }
}
