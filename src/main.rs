mod config;
mod error;
mod mcp;
mod memory;

use clap::{Parser, Subcommand};
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::McpServer;
use rust_mcp_sdk::mcp_server::server_runtime;
use rust_mcp_sdk::mcp_server::McpServerOptions;
use rust_mcp_sdk::StdioTransport;
use rust_mcp_sdk::ToMcpServerHandler;
use rust_mcp_sdk::TransportOptions;
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "total-recall")]
#[command(about = "Agentic memory MCP server with SQLite + vector search")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP server
    Serve {
        #[arg(long, default_value = "1000")]
        timeout: u64,
    },
    /// Write a new note (or append to today's note if it already exists)
    Write {
        #[arg(required = true)]
        content: String,

        #[arg(long)]
        timestamp: Option<String>,

        /// Append to today's note if it already exists (default: false = create only)
        #[arg(long, default_value_t = false)]
        append: bool,
    },
    /// Read a note by date
    Read {
        #[arg(required = true)]
        date: String,
    },
    /// Search notes semantically
    Search {
        #[arg(required = true)]
        query: String,

        #[arg(long, default_value = "10")]
        limit: usize,

        #[arg(long)]
        include_archived: bool,
    },
    /// Get recent notes
    Recent {
        #[arg(long, default_value = "10")]
        limit: usize,

        #[arg(long, default_value = "7")]
        days: usize,

        #[arg(long)]
        include_archived: bool,
    },
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let cli = Cli::parse();

    let config_path = cli.config.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".total-recall")
            .join("config.yaml")
    });

    let config = match config::Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to load config: {}", e);
            return Err(rust_mcp_sdk::error::McpSdkError::Internal {
                description: format!("Failed to load config: {}", e)
            });
        }
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "total_recall=info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Loading Total-Recall from {:?}", config_path);
    tracing::info!("Memory directory: {:?}", config.memory_dir);
    tracing::info!("Database path: {:?}", config.db_path);

    match cli.command {
        Some(Commands::Serve { .. }) | None => {
            run_mcp_server(&config).await?;
        }
        Some(Commands::Write { content, timestamp, append }) => {
            run_write(&config, &content, timestamp.as_deref(), append).await?;
        }
        Some(Commands::Read { date }) => {
            run_read(&config, &date).await?;
        }
        Some(Commands::Search { query, limit, include_archived }) => {
            run_search(&config, &query, limit, include_archived).await?;
        }
        Some(Commands::Recent {
            limit,
            days,
            include_archived,
        }) => {
            run_recent(&config, limit, days, include_archived).await?;
        }
    }

    Ok(())
}

async fn run_mcp_server(config: &config::Config) -> SdkResult<()> {
    let store = match memory::store::MemoryStore::new(&config.db_path) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Failed to initialize database at {:?}: {}", config.db_path, e);
            tracing::error!("{}", msg);
            return Err(rust_mcp_sdk::error::McpSdkError::Internal {
                description: msg,
            });
        }
    };

    tracing::info!("Starting MCP server via stdio...");

    let server = match mcp::server::MemoryMcpServer::new(store, config.memory_dir.clone()) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Failed to create server: {}", e);
            tracing::error!("{}", msg);
            return Err(rust_mcp_sdk::error::McpSdkError::Internal {
                description: msg,
            });
        }
    };

    let server_details = rust_mcp_sdk::schema::InitializeResult {
        server_info: rust_mcp_sdk::schema::Implementation {
            name: "total-recall".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Total Recall - Agentic Memory Server".into()),
            description: Some("Memory storage with semantic search".into()),
            icons: vec![],
            website_url: None,
        },
        capabilities: rust_mcp_sdk::schema::ServerCapabilities {
            tools: Some(rust_mcp_sdk::schema::ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: rust_mcp_sdk::schema::ProtocolVersion::V2025_11_25.into(),
        instructions: None,
        meta: None,
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = server.to_mcp_server_handler();
    let server_runtime = server_runtime::create_server(McpServerOptions {
        transport,
        handler,
        server_details,
        task_store: None,
        client_task_store: None,
    });

    tracing::info!("Memory store initialized at {:?}", config.db_path);
    server_runtime.start().await
}

async fn run_write(
    config: &config::Config,
    content: &str,
    timestamp: Option<&str>,
    append: bool,
) -> SdkResult<()> {
    let store = match memory::store::MemoryStore::new(&config.db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize store: {}", e);
            std::process::exit(1);
        }
    };

    let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
    let final_content = if let Some(ts) = timestamp {
        format!("## {}\n\n{}", ts, content)
    } else {
        format!("\n{}", content)
    };

    if append {
        match store.append_note(&current_date, &final_content) {
            Ok(note) => {
                println!("Appended to note for {}", note.date);
            }
            Err(e) => {
                eprintln!("Error appending to note: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        match store.create_note(&current_date, &final_content) {
            Ok(note) => {
                println!("Created note for {}", note.date);
                println!("Title: {}", note.metadata.title.as_deref().unwrap_or("Untitled"));
            }
            Err(error::MemoryError::FileExistsError(_)) => {
                // Auto-append if note already exists — don't fail
                match store.append_note(&current_date, &final_content) {
                    Ok(note) => {
                        println!("Appended to existing note for {}", note.date);
                    }
                    Err(e) => {
                        eprintln!("Error appending to note: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error creating note: {}", e);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

async fn run_read(config: &config::Config, date: &str) -> SdkResult<()> {
    let store = match memory::store::MemoryStore::new(&config.db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize store: {}", e);
            std::process::exit(1);
        }
    };

    match store.read_note(date) {
        Ok(note) => {
            println!("## {}\n", note.date);
            println!("{}", note.content);

            if !note.observations.is_empty() {
                println!("\n### Observations:");
                for obs in &note.observations {
                    let category = obs.category.as_deref().unwrap_or("note");
                    println!("- [`{}`] {}: {}", category, obs.timestamp, obs.content);
                    if !obs.tags.is_empty() {
                        println!("  Tags: {}", obs.tags.join(", "));
                    }
                }
            }

            Ok(())
        }
        Err(error::MemoryError::NotFound(_)) => {
            eprintln!("No note found for date: {}", date);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error reading note: {}", e);
            std::process::exit(1);
        }
    }
}

async fn run_search(
    config: &config::Config,
    query: &str,
    limit: usize,
    include_archived: bool,
) -> SdkResult<()> {
    let embedder = match memory::embedder::Embedder::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to initialize embedder: {}", e);
            std::process::exit(1);
        }
    };
    let store = match memory::store::MemoryStore::new(&config.db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize store: {}", e);
            std::process::exit(1);
        }
    };

    let query_embedding = embedder.embed(query);

    match store.search_notes(&query_embedding, limit, include_archived) {
        Ok(notes) => {
            if notes.is_empty() {
                println!("No notes found matching your query.");
            } else {
                for note in &notes {
                    let title = note.metadata.title.as_deref().unwrap_or("Untitled");
                    println!("### {} - {}\n", note.date, title);
                    let content_preview = note.content.chars().take(200).collect::<String>();
                    println!("{}\n", content_preview);
                }
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error searching: {}", e);
            std::process::exit(1);
        }
    }
}

async fn run_recent(
    config: &config::Config,
    limit: usize,
    days: usize,
    include_archived: bool,
) -> SdkResult<()> {
    let store = match memory::store::MemoryStore::new(&config.db_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize store: {}", e);
            std::process::exit(1);
        }
    };

    match store.get_recent_notes(limit, days, include_archived) {
        Ok(notes) => {
            if notes.is_empty() {
                println!("No notes found in the last {} days.", days);
            } else {
                println!("Recent notes (last {} days):\n", days);
                for note in &notes {
                    let title = note.metadata.title.as_deref().unwrap_or(&note.date);
                    println!("- **{}** ({})", note.date, title);
                }
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Error getting recent notes: {}", e);
            std::process::exit(1);
        }
    }
}
