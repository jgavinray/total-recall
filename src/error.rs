use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MemoryError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("File not found: {0}")]
    FileNotFoundError(PathBuf),

    #[error("File already exists: {0}")]
    FileExistsError(PathBuf),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("IO error: {0}")]
    IO(#[from] std::io::Error),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    #[error("Date format error: expected mm/dd/yyyy, got {0}")]
    DateParse(String),

    #[error("MCP error: {0}")]
    McpError(String),

    #[error("Config error: {0}")]
    ConfigError(String),
}

pub type Result<T> = std::result::Result<T, MemoryError>;
