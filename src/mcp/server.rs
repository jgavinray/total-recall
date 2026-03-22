use crate::memory::embedder::Embedder;
use crate::memory::store::MemoryStore;
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::wrapper::Parameters,
    model::*,
    schemars,
    tool, tool_handler, tool_router,
};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WriteNoteParams {
    /// Content to store as a memory note
    pub content: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadNoteParams {
    /// Date in mm-dd-yyyy format (e.g. 03-22-2026)
    pub date: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchNotesParams {
    /// Semantic search query
    pub query: String,
    /// Maximum number of results (default: 10)
    #[serde(default)]
    pub limit: Option<usize>,
    /// Include archived notes
    #[serde(default)]
    pub include_archived: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RecentNotesParams {
    /// Number of days to look back (default: 7)
    #[serde(default)]
    pub days: Option<i32>,
    /// Maximum number of notes to return (default: 10)
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone)]
pub struct MemoryMcpServer {
    store: Arc<RwLock<MemoryStore>>,
    embedder: Arc<Embedder>,
    tool_router: rmcp::handler::server::router::tool::ToolRouter<MemoryMcpServer>,
}

#[tool_router]
impl MemoryMcpServer {
    pub fn new(
        store: MemoryStore,
        _memory_dir: std::path::PathBuf,
    ) -> std::result::Result<Self, crate::error::MemoryError> {
        let embedder = Embedder::new()?;
        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            embedder: Arc::new(embedder),
            tool_router: Self::tool_router(),
        })
    }

    #[tool(description = "Create or append a new memory note for today's date")]
    async fn write_note(
        &self,
        Parameters(params): Parameters<WriteNoteParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
        let store = self.store.read().await;
        match store.append_note(&current_date, &params.content) {
            Ok(_) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Stored memory for {}",
                current_date
            ))])),
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Read a memory note by date (format: mm-dd-yyyy)")]
    async fn read_note(
        &self,
        Parameters(params): Parameters<ReadNoteParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let store = self.store.read().await;
        match store.read_note(&params.date) {
            Ok(note) => {
                let content = format!("## {}\n\n{}", note.date, note.content);
                Ok(CallToolResult::success(vec![Content::text(content)]))
            }
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Search memory notes semantically")]
    async fn search_notes(
        &self,
        Parameters(params): Parameters<SearchNotesParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let limit = params.limit.unwrap_or(10);
        let include_archived = params.include_archived.unwrap_or(false);
        let query_embedding = self.embedder.embed(&params.query);
        let store = self.store.read().await;
        match store.search_notes(&query_embedding, limit, include_archived) {
            Ok(notes) => {
                if notes.is_empty() {
                    Ok(CallToolResult::success(vec![Content::text(
                        "No notes found matching your query.",
                    )]))
                } else {
                    let content = notes
                        .iter()
                        .map(|n| {
                            let preview = n.content.chars().take(300).collect::<String>();
                            format!("### {}\n{}\n", n.date, preview)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(CallToolResult::success(vec![Content::text(content)]))
                }
            }
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }

    #[tool(description = "Get recent memory notes from the last N days")]
    async fn recent_notes(
        &self,
        Parameters(params): Parameters<RecentNotesParams>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let days = params.days.unwrap_or(7) as usize;
        let limit = params.limit.unwrap_or(10);
        let store = self.store.read().await;
        match store.get_recent_notes(limit, days, false) {
            Ok(notes) => {
                if notes.is_empty() {
                    Ok(CallToolResult::success(vec![Content::text(format!(
                        "No notes found in the last {} days.",
                        days
                    ))]))
                } else {
                    let content = notes
                        .iter()
                        .map(|n| {
                            let title = n.metadata.title.as_deref().unwrap_or(&n.date);
                            format!("- **{}** ({})", n.date, title)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(CallToolResult::success(vec![Content::text(content)]))
                }
            }
            Err(e) => Ok(CallToolResult::success(vec![Content::text(format!(
                "Error: {}",
                e
            ))])),
        }
    }
}

#[tool_handler]
impl ServerHandler for MemoryMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder().enable_tools().build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_protocol_version(ProtocolVersion::V_2024_11_05)
        .with_instructions(
            "Agentic memory MCP server. Tools: write_note, read_note, search_notes, recent_notes."
                .to_string(),
        )
    }
}
