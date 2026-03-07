use crate::error::Result;
use crate::memory::embedder::Embedder;
use crate::memory::store::MemoryStore;
use crate::memory::models::{Note, NoteMetadata};
use async_trait::async_trait;
use mcp::server::{Server, ServerHandler};
use mcp::tools::{ListToolsResult, CallToolResult, Tool};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct MemoryMcpServer {
    store: Arc<RwLock<MemoryStore>>,
    embedder: Arc<Embedder>,
    memory_dir: std::path::PathBuf,
}

impl MemoryMcpServer {
    pub fn new(store: MemoryStore, memory_dir: std::path::PathBuf) -> Result<Self> {
        let embedder = Embedder::new()?;
        Ok(Self {
            store: Arc::new(RwLock::new(store)),
            embedder: Arc::new(embedder),
            memory_dir,
        })
    }

    pub async fn list_tools(&self) -> ListToolsResult {
        let tools = vec![
            Tool {
                name: "write_note".to_string(),
                description: "Create a new memory note. Returns error if date already exists (immutability enforced).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The note content in markdown format"
                        },
                        "timestamp": {
                            "type": "string",
                            "description": "Optional timestamp section header (e.g., '14:30')"
                        }
                    },
                    "required": ["content"]
                }),
            },
            Tool {
                name: "read_note".to_string(),
                description: "Read a note by date in mm/dd/yyyy format".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "date": {
                            "type": "string",
                            "description": "Date in mm/dd/yyyy format"
                        }
                    },
                    "required": ["date"]
                }),
            },
            Tool {
                name: "search_notes".to_string(),
                description: "Search notes using semantic vector similarity".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query text"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results",
                            "default": 10
                        },
                        "include_archived": {
                            "type": "boolean",
                            "description": "Include archived notes",
                            "default": false
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "recent_notes".to_string(),
                description: "Get notes from the last N days, excluding archived by default".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "default": 10
                        },
                        "days": {
                            "type": "integer",
                            "default": 7
                        },
                        "include_archived": {
                            "type": "boolean",
                            "default": false
                        }
                    }
                }),
            },
            Tool {
                name: "build_context".to_string(),
                description: "Get all observations from a specific date with semantic context".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "date": {
                            "type": "string",
                            "description": "Date in mm/dd/yyyy format"
                        },
                        "category_filter": {
                            "type": "string",
                            "description": "Optional filter by category (decision, action, note, idea, question, risk)"
                        }
                    },
                    "required": ["date"]
                }),
            },
        ];

        ListToolsResult { tools }
    }
}

#[async_trait]
impl ServerHandler for MemoryMcpServer {
    type Error = crate::error::MemoryError;
    type ToolExecutor = Server<Self>;

    async fn initialize(&self, _client_version: &str) -> mcp::protocol::InitializeResult {
        mcp::protocol::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            server_info: mcp::protocol::Implementation {
                name: "total-recall".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: mcp::protocol::ServerCapabilities {
                tools: Some(mcp::protocol::ToolsCapability::ListChanged),
                ..Default::default()
            },
            instructions: Some("Agentic memory system. Use write_note to record thoughts, search_notes for semantic search, recent_notes for recent activity.".to_string()),
        }
    }

    async fn list_tools(&self) -> ListToolsResult {
        self.list_tools().await
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<CallToolResult, Self::Error> {
        match name {
            "write_note" => self.call_write_note(arguments).await,
            "read_note" => self.call_read_note(arguments).await,
            "search_notes" => self.call_search_notes(arguments).await,
            "recent_notes" => self.call_recent_notes(arguments).await,
            "build_context" => self.call_build_context(arguments).await,
            _ => Err(crate::error::MemoryError::NotFound(format!("Unknown tool: {}", name))),
        }
    }
}

impl MemoryMcpServer {
    async fn call_write_note(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Invalid arguments: expected object".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Missing required field: content".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let timestamp = args.get("timestamp").and_then(|v| v.as_str());
        let current_date = chrono::Utc::now().format("%m/%d/%Y").to_string();

        let final_content = if let Some(ts) = timestamp {
            format!("## {}\n\n{}", ts, content)
        } else {
            format!("\n{}", content)
        };

        let store = self.store.read().await;

        match store.create_note(&current_date, &final_content) {
            Ok(_) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Successfully created note for {}", current_date),
                }],
                is_error: Some(false),
            }),
            Err(crate::error::MemoryError::FileExistsError(_)) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Note for {} already exists (immutability enforced). Use append or a different date.", current_date),
                }],
                is_error: Some(true),
            }),
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error creating note: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }

    async fn call_read_note(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Invalid arguments: expected object".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let date = match args.get("date").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Missing required field: date".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let store = self.store.read().await;

        match store.read_note(date) {
            Ok(note) => {
                let mut output = format!("## {}\n\n{}\n\n", note.date, note.content);

                if !note.observations.is_empty() {
                    output.push_str("### Observations:\n");
                    for obs in &note.observations {
                        let category = obs.category.as_deref().unwrap_or("note");
                        output.push_str(&format!("- [`{}] {} ({}): {}\n", category, obs.timestamp, obs.section.as_deref().unwrap_or("general"), obs.content));
                        if !obs.tags.is_empty() {
                            output.push_str(&format!("  Tags: {}\n", obs.tags.join(", ")));
                        }
                    }
                }

                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text { text: output }],
                    is_error: Some(false),
                })
            },
            Err(crate::error::MemoryError::NotFound(_)) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("No note found for date: {}", date),
                }],
                is_error: Some(true),
            }),
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error reading note: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }

    async fn call_search_notes(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Invalid arguments: expected object".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Missing required field: query".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(10);
        let include_archived = args.get("include_archived").and_then(|v| v.as_bool()).unwrap_or(false);

        let query_embedding = self.embedder.embed(query);

        let store = self.store.read().await;

        match store.search_notes(&query_embedding, limit, include_archived) {
            Ok(notes) => {
                let mut output = String::new();
                if notes.is_empty() {
                    output = "No notes found matching your query.".to_string();
                } else {
                    for note in &notes {
                        output.push_str(&format!(
                            "### {} - {}\n{}\n\n",
                            note.date,
                            note.metadata.title.as_deref().unwrap_or("Untitled"),
                            note.content.chars().take(300).collect::<String>()
                        ));
                    }
                }

                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text { text: output }],
                    is_error: Some(false),
                })
            },
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error searching: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }

    async fn call_recent_notes(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Invalid arguments: expected object".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(10);
        let days = args.get("days").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(7);
        let include_archived = args.get("include_archived").and_then(|v| v.as_bool()).unwrap_or(false);

        let store = self.store.read().await;

        match store.get_recent_notes(limit, days, include_archived) {
            Ok(notes) => {
                let mut output = String::new();
                if notes.is_empty() {
                    output = format!("No notes found in the last {} days.", days);
                } else {
                    output.push_str(&format!("Recent notes (last {} days):\n\n", days));
                    for note in &notes {
                        let title = note.metadata.title.as_deref().unwrap_or(&note.date);
                        output.push_str(&format!("- **{}** ({})\n", note.date, title));
                    }
                }

                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text { text: output }],
                    is_error: Some(false),
                })
            },
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error getting recent notes: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }

    async fn call_build_context(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Invalid arguments: expected object".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let date = match args.get("date").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: "Missing required field: date".to_string(),
                }],
                is_error: Some(true),
            }),
        };

        let category_filter = args.get("category_filter").and_then(|v| v.as_str());

        let store = self.store.read().await;
        let note = match store.read_note(date) {
            Ok(n) => n,
            Err(crate::error::MemoryError::NotFound(_)) => {
                return Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text {
                        text: format!("No note found for date: {}", date),
                    }],
                    is_error: Some(true),
                })
            },
            Err(e) => {
                return Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text {
                        text: format!("Error reading note: {}", e),
                    }],
                    is_error: Some(true),
                })
            },
        };

        let mut output = format!("## Context for {}\n\n", date);
        output.push_str(&format!("Full content:\n```\n{}\n```\n\n", note.content));

        output.push_str("### Observations:\n");

        for obs in &note.observations {
            if let Some(ref filter) = category_filter {
                if obs.category.as_deref() != Some(filter) {
                    continue;
                }
            }

            let category = obs.category.as_deref().unwrap_or("note");
            output.push_str(&format!("\n#### {} ({})\n", category, obs.timestamp));

            if let Some(section) = &obs.section {
                output.push_str(&format!("Section: {}\n", section));
            }

            output.push_str(&format!("Content: {}\n", obs.content));

            if !obs.tags.is_empty() {
                output.push_str(&format!("Tags: {}\n", obs.tags.join(", ")));
            }
        }

        if note.observations.is_empty() {
            output.push_str("No observations found in this note.\n");
        }

        Ok(CallToolResult {
            content: vec![mcp::protocol::ContentPart::Text { text: output }],
            is_error: Some(false),
        })
    }
}
