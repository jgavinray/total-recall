mod config;
mod error;
mod mcp;
mod memory;

use clap::{Parser, Subcommand};
use rust_mcp_sdk::mcp_server::server_runtime::ServerRuntime;
use rust_mcp_sdk::server_runtime::stdio::StdioTransport;
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
    /// Write a new note
    Write {
        #[arg(required = true)]
        content: String,

        #[arg(long)]
        timestamp: Option<String>,
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
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let config_path = cli.config.clone().unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".total-recall")
            .join("config.yaml")
    });

    let config = config::Config::load(&config_path)?;

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
        Some(Commands::Serve { .. }) => {
            run_mcp_server(&config).await?;
        }
        Some(Commands::Write { content, timestamp }) => {
            run_write(&config, &content, timestamp.as_deref()).await?;
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
        None => {
            run_mcp_server(&config).await?;
        }
    }

    Ok(())
}

async fn run_mcp_server(config: &config::Config) -> Result<(), Box<dyn std::error::Error>> {
    match memory::store::MemoryStore::new(&config.db_path) {
        Ok(store) => {
            tracing::info!("Starting MCP server via stdio...");

            let server = mcp::server::MemoryMcpServer::new(store, config.memory_dir.clone())?;
            let runtime = ServerRuntime::new(server, StdioTransport::new());

            tracing::info!("Memory store initialized at {:?}", config.db_path);
            runtime.run().await?;
        }
        Err(e) => {
            eprintln!("Failed to initialize database at {:?}: {}", config.db_path, e);
        }
    }

    Ok(())
}

async fn run_write(
    config: &config::Config,
    content: &str,
    timestamp: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = memory::store::MemoryStore::new(&config.db_path)?;

    let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
    let final_content = if let Some(ts) = timestamp {
        format!("## {}\n\n{}", ts, content)
    } else {
        format!("\n{}", content)
    };

    match store.create_note(&current_date, &final_content) {
        Ok(note) => {
            println!("Created note for {}", note.date);
            println!("Title: {}", note.metadata.title.as_deref().unwrap_or("Untitled"));
        }
        Err(error::MemoryError::FileExistsError(_)) => {
            eprintln!("Note for {} already exists (immutability enforced)", current_date);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error creating note: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn run_read(config: &config::Config, date: &str) -> Result<(), Box<dyn std::error::Error>> {
    let store = memory::store::MemoryStore::new(&config.db_path)?;

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
) -> Result<(), Box<dyn std::error::Error>> {
    let embedder = memory::embedder::Embedder::new()?;
    let store = memory::store::MemoryStore::new(&config.db_path)?;

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
) -> Result<(), Box<dyn std::error::Error>> {
    let store = memory::store::MemoryStore::new(&config.db_path)?;

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
