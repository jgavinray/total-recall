mod config;
mod error;
mod mcp;
mod memory;

use clap::{Parser, Subcommand};
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
        /// Transport mode: "stdio" or "http"
        #[arg(long, default_value = "stdio")]
        transport: String,

        /// Port for HTTP transport
        #[arg(long, default_value = "8811")]
        port: u16,

        /// Host/address to bind for HTTP transport
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
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
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config_path = cli.config.clone().unwrap_or_else(|| {
        // Also check env var TOTAL_RECALL_CONFIG
        if let Ok(p) = std::env::var("TOTAL_RECALL_CONFIG") {
            return PathBuf::from(p);
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".total-recall")
            .join("config.yaml")
    });

    let config = match config::Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            config::Config::default()
        }
    };

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "total_recall=info".to_string().into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    tracing::info!("Loading Total-Recall from {:?}", config_path);
    tracing::info!("Memory directory: {:?}", config.memory_dir);
    tracing::info!("Database path: {:?}", config.db_path);

    match cli.command {
        Some(Commands::Serve { transport, port, host }) => {
            run_mcp_server(&config, &transport, port, &host).await?;
        }
        None => {
            // Default: run stdio server
            run_mcp_server(&config, "stdio", 8811, "127.0.0.1").await?;
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
        Some(Commands::Recent { limit, days, include_archived }) => {
            run_recent(&config, limit, days, include_archived).await?;
        }
    }

    Ok(())
}

async fn run_mcp_server(
    config: &config::Config,
    transport: &str,
    port: u16,
    host: &str,
) -> anyhow::Result<()> {
    // Set env var so Embedder::cache_dir() picks up config's model cache path
    // SAFETY: single-threaded at this point; no other threads reading env
    unsafe {
        std::env::set_var("TR_MODEL_CACHE_DIR", &config.embedding.cache_dir);
    }

    let store = memory::store::MemoryStore::new(&config.db_path).map_err(|e| {
        anyhow::anyhow!("Failed to initialize database at {:?}: {}", config.db_path, e)
    })?;

    let server = mcp::server::MemoryMcpServer::new(store, config.memory_dir.clone())
        .map_err(|e| anyhow::anyhow!("Failed to create server: {}", e))?;

    match transport {
        "http" => {
            use rmcp::transport::streamable_http_server::{
                StreamableHttpServerConfig, StreamableHttpService,
                session::local::LocalSessionManager,
            };

            let bind_addr = format!("{}:{}", host, port);
            tracing::info!("Starting Streamable HTTP MCP server on {}", bind_addr);

            let ct = tokio_util::sync::CancellationToken::new();
            let ct_clone = ct.clone();

            let service = StreamableHttpService::new(
                move || Ok(server.clone()),
                LocalSessionManager::default().into(),
                StreamableHttpServerConfig {
                    cancellation_token: ct.child_token(),
                    ..Default::default()
                },
            );

            let router = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
            tracing::info!("total-recall HTTP MCP server listening on {}", bind_addr);

            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    tokio::signal::ctrl_c()
                        .await
                        .expect("failed to install CTRL+C handler");
                    ct_clone.cancel();
                })
                .await?;
        }
        _ => {
            use rmcp::{ServiceExt, transport::stdio};
            tracing::info!("Starting stdio MCP server...");
            let service = server.serve(stdio()).await?;
            service.waiting().await?;
        }
    }

    Ok(())
}

async fn run_write(
    config: &config::Config,
    content: &str,
    timestamp: Option<&str>,
    append: bool,
) -> anyhow::Result<()> {
    let store = memory::store::MemoryStore::new(&config.db_path)
        .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    let current_date = chrono::Utc::now().format("%m-%d-%Y").to_string();
    let final_content = if let Some(ts) = timestamp {
        format!("## {}\n\n{}", ts, content)
    } else {
        format!("\n{}", content)
    };

    if append {
        match store.append_note(&current_date, &final_content) {
            Ok(note) => println!("Appended to note for {}", note.date),
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
                match store.append_note(&current_date, &final_content) {
                    Ok(note) => println!("Appended to existing note for {}", note.date),
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

async fn run_read(config: &config::Config, date: &str) -> anyhow::Result<()> {
    let store = memory::store::MemoryStore::new(&config.db_path)
        .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

    match store.read_note(date) {
        Ok(note) => {
            println!("## {}\n", note.date);
            println!("{}", note.content);
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

    Ok(())
}

async fn run_search(
    config: &config::Config,
    query: &str,
    limit: usize,
    include_archived: bool,
) -> anyhow::Result<()> {
    unsafe {
        std::env::set_var("TR_MODEL_CACHE_DIR", &config.embedding.cache_dir);
    }
    let embedder = memory::embedder::Embedder::new()
        .map_err(|e| anyhow::anyhow!("Failed to initialize embedder: {}", e))?;
    let store = memory::store::MemoryStore::new(&config.db_path)
        .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

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
        }
        Err(e) => {
            eprintln!("Error searching: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn run_recent(
    config: &config::Config,
    limit: usize,
    days: usize,
    include_archived: bool,
) -> anyhow::Result<()> {
    let store = memory::store::MemoryStore::new(&config.db_path)
        .map_err(|e| anyhow::anyhow!("Failed to initialize store: {}", e))?;

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
        }
        Err(e) => {
            eprintln!("Error getting recent notes: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
