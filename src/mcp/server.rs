use crate::error::Result;
use crate::memory::embedder::Embedder;
use crate::memory::store::MemoryStore;
use async_trait::async_trait;
use rust_mcp_sdk::mcp_server::ServerHandler;
use rust_mcp_sdk::schema::{Tool, ToolInputSchema, ToolResult};
use rust_mcp_sdk::server_runtime::server_runtime::ServerRuntime;
use rust_mcp_sdk::server_runtime::stdio::StdioTransport;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
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

    fn create_tools() -> Vec<Tool> {
        vec![
            Tool {
                name: "write_note".to_string(),
                description: Some("Create a new memory note. Returns error if date already exists (immutability enforced).".to_string()),
                input_schema: Self::create_tool_input_schema(vec!["content"], Some(Self::create_properties())),
                ..Default::default()
            },
            Tool {
                name: "read_note".to_string(),
                description: Some("Read a note by date in mm-dd-yyyy format".to_string()),
                input_schema: Self::create_tool_input_schema(vec!["date"], Some(Self::create_date_properties())),
                ..Default::default()
            },
            Tool {
                name: "search_notes".to_string(),
                description: Some("Search notes using semantic vector similarity".to_string()),
                input_schema: Self::create_tool_input_schema(vec!["query"], Some(Self::create_search_properties())),
                ..Default::default()
            },
            Tool {
                name: "recent_notes".to_string(),
                description: Some("Get notes from the last N days, excluding archived by default".to_string()),
                input_schema: Self::create_tool_input_schema(vec![], Some(Self::create_recent_properties())),
                ..Default::default()
            },
            Tool {
                name: "build_context".to_string(),
                description: Some("Get all observations from a specific date with semantic context".to_string()),
                input_schema: Self::create_tool_input_schema(vec!["date"], Some(Self::create_context_properties())),
                ..Default::default()
            },
        ]
    }

    fn create_properties() -> HashMap<String, Map<String, Value>> {
        let mut properties = Map::new();
        properties.insert(
            "content".to_string(),
            json!({
                "type": "string",
                "description": "The note content in markdown format"
            }),
        );
        properties.insert(
            "timestamp".to_string(),
            json!({
                "type": "string",
                "description": "Optional timestamp section header (e.g., '14:30')"
            }),
        );
        HashMap::from([("content".to_string(), json!({"type": "string", "description": "The note content in markdown format"}))])
    }

    fn create_date_properties() -> HashMap<String, Map<String, Value>> {
        HashMap::from([("date".to_string(), json!({"type": "string", "description": "Date in mm-dd-yyyy format"}))])
    }

    fn create_search_properties() -> HashMap<String, Map<String, Value>> {
        HashMap::from([
            ("query".to_string(), json!({"type": "string", "description": "Search query text"})),
            ("limit".to_string(), json!({"type": "integer", "description": "Maximum number of results", "default": 10})),
            ("include_archived".to_string(), json!({"type": "boolean", "description": "Include archived notes", "default": false})),
        ])
    }

    fn create_recent_properties() -> HashMap<String, Map<String, Value>> {
        HashMap::from([
            ("limit".to_string(), json!({"type": "integer", "default": 10})),
            ("days".to_string(), json!({"type": "integer", "default": 7})),
            ("include_archived".to_string(), json!({"type": "boolean", "default": false})),
        ])
    }

    fn create_context_properties() -> HashMap<String, Map<String, Value>> {
        HashMap::from([
            ("date".to_string(), json!({"type": "string", "description": "Date in mm-dd-yyyy format"})),
            ("category_filter".to_string(), json!({"type": "string", "description": "Optional filter by category (decision, action, note, idea, question, risk)"})),
        ])
    }

    fn create_tool_input_schema(
        required: Vec<String>,
        properties: Option<HashMap<String, Map<String, Value>>>,
    ) -> ToolInputSchema {
        ToolInputSchema::new(
            required,
            properties,
            Some("https://json-schema.org/draft/2020-12/schema".to_string()),
        )
    }
}

#[async_trait]
impl ServerHandler for MemoryMcpServer {
    fn name(&self) -> &'static str {
        "total-recall"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn description(&self) -> &'static str {
        "Agentic memory system. Use write_note to record thoughts, search_notes for semantic search, recent_notes for recent activity."
    }

    async fn list_tools(&self) -> ToolResult {
        Ok(Self::create_tools())
    }
}

impl MemoryMcpServer {
    async fn call_write_note(&self, arguments: Value) -> String {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return "Invalid arguments: expected object".to_string(),
        };

        let content = match args.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return "Missing required field: content".to_string(),
        };

        let timestamp = args.get("timestamp").and_then(|v| v.as_str());
        let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();

        let final_content = if let Some(ts) = timestamp {
            format!("## {}\n\n{}", ts, content)
        } else {
            format!("\n{}", content)
        };

        let store = self.store.read().await;

        match store.create_note(&current_date, &final_content) {
            Ok(_) => format!("Successfully created note for {}", current_date),
            Err(crate::error::MemoryError::FileExistsError(_)) => {
                format!("Note for {} already exists (immutability enforced). Use append or a different date.", current_date)
            },
            Err(e) => format!("Error creating note: {}", e),
        }
    }

    async fn call_read_note(&self, arguments: Value) -> String {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return "Invalid arguments: expected object".to_string(),
        };

        let date = match args.get("date").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return "Missing required field: date".to_string(),
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

                output
            },
            Err(crate::error::MemoryError::NotFound(_)) => {
                format!("No note found for date: {}", date)
            },
            Err(e) => format!("Error reading note: {}", e),
        }
    }

    async fn call_search_notes(&self, arguments: Value) -> String {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return "Invalid arguments: expected object".to_string(),
        };

        let query = match args.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return "Missing required field: query".to_string(),
        };

        let limit = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(10);
        let include_archived = args.get("include_archived").and_then(|v| v.as_bool()).unwrap_or(false);

        let query_embedding = self.embedder.embed(query);

        let store = self.store.read().await;

        match store.search_notes(&query_embedding, limit, include_archived) {
            Ok(notes) => {
                let mut output = String::new();
                if notes.is_empty() {
                    "No notes found matching your query.".to_string()
                } else {
                    for note in &notes {
                        output.push_str(&format!(
                            "### {} - {}\n{}\n\n",
                            note.date,
                            note.metadata.title.as_deref().unwrap_or("Untitled"),
                            note.content.chars().take(300).collect::<String>()
                        ));
                    }
                    output
                }
            },
            Err(e) => format!("Error searching: {}", e),
        }
    }

    async fn call_recent_notes(&self, arguments: Value) -> String {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return "Invalid arguments: expected object".to_string(),
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
                output
            },
            Err(e) => format!("Error getting recent notes: {}", e),
        }
    }

    async fn call_build_context(&self, arguments: Value) -> String {
        let args = match arguments.as_object() {
            Some(obj) => obj,
            None => return "Invalid arguments: expected object".to_string(),
        };

        let date = match args.get("date").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return "Missing required field: date".to_string(),
        };

        let category_filter = args.get("category_filter").and_then(|v| v.as_str());

        let store = self.store.read().await;
        let note = match store.read_note(date) {
            Ok(n) => n,
            Err(crate::error::MemoryError::NotFound(_)) => {
                return format!("No note found for date: {}", date)
            },
            Err(e) => {
                return format!("Error reading note: {}", e)
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

        output
    }
}
