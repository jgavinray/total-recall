use clap::{Parser, Subcommand};
use std::path::PathBuf;
use total_recall::{config, mcp, memory};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod command_handlers;
mod http_api;
use command_handlers::{run_read, run_recent, run_search, run_write};
use http_api::{MemoryApiState, api_recent_notes, api_search_notes};

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

    let config = config::Config::load(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to load required config {:?}: {}", config_path, e))?;

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
    tracing::info!(
        model = %config.embedding.model,
        dimension = config.embedding.dimension,
        "Embedding model configured"
    );

    match cli.command {
        Some(Commands::Serve {
            transport,
            port,
            host,
        }) => {
            run_mcp_server(&config, &transport, port, &host).await?;
        }
        None => {
            // Default: run stdio server
            run_mcp_server(&config, "stdio", 8811, "127.0.0.1").await?;
        }
        Some(Commands::Write {
            content,
            timestamp,
            append,
        }) => {
            run_write(&config, &content, timestamp.as_deref(), append).await?;
        }
        Some(Commands::Read { date }) => {
            run_read(&config, &date).await?;
        }
        Some(Commands::Search {
            query,
            limit,
            include_archived,
        }) => {
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

async fn run_mcp_server(
    config: &config::Config,
    transport: &str,
    port: u16,
    host: &str,
) -> anyhow::Result<()> {
    let store =
        memory::store::MemoryStore::new_with_embedding_config(&config.db_path, &config.embedding)
            .map_err(|e| {
            anyhow::anyhow!(
                "Failed to initialize database at {:?}: {}",
                config.db_path,
                e
            )
        })?;
    let shared_store = std::sync::Arc::new(tokio::sync::RwLock::new(store));
    let api_state = MemoryApiState {
        store: shared_store.clone(),
    };

    let server = mcp::server::MemoryMcpServer::from_shared(shared_store);

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

            let mut config = StreamableHttpServerConfig::default();
            config.cancellation_token = ct.child_token();

            let service = StreamableHttpService::new(
                move || Ok(server.clone()),
                LocalSessionManager::default().into(),
                config,
            );

            let router = axum::Router::new()
                .route("/api/recent", axum::routing::get(api_recent_notes))
                .route("/api/search", axum::routing::post(api_search_notes))
                .nest_service("/mcp", service)
                .with_state(api_state);
            let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
            tracing::info!(
                "total-recall HTTP MCP/API server listening on {}",
                bind_addr
            );

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
