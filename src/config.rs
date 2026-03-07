use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Directory containing all memory files (organized by mm-yyyy/ subdirectories)
    #[serde(default = "default_memory_dir")]
    pub memory_dir: PathBuf,

    /// Path to SQLite database file
    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,

    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Embedding model settings
    #[serde(default)]
    pub embedding: EmbeddingConfig,

    /// Search settings
    #[serde(default)]
    pub search: SearchConfig,

    /// MCP server settings
    #[serde(default)]
    pub mcp: McpConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            memory_dir: default_memory_dir(),
            db_path: default_db_path(),
            logging: LoggingConfig::default(),
            embedding: EmbeddingConfig::default(),
            search: SearchConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

fn default_memory_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".total-recall")
}

fn default_db_path() -> PathBuf {
    default_memory_dir().join("memory.db")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default = "default_log_file")]
    pub file: PathBuf,

    #[serde(default = "default_max_size_mb")]
    pub max_size_mb: u32,

    #[serde(default = "default_backup_count")]
    pub backup_count: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            file: default_log_file(),
            max_size_mb: default_max_size_mb(),
            backup_count: default_backup_count(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> PathBuf {
    default_memory_dir().join("logs").join("server.log")
}

fn default_max_size_mb() -> u32 {
    10
}

fn default_backup_count() -> u32 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default)]
    pub model: String,

    #[serde(default = "default_dimension")]
    pub dimension: usize,

    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            dimension: default_dimension(),
            cache_dir: default_cache_dir(),
        }
    }
}

fn default_model() -> String {
    "sentence-transformers/all-MiniLM-L6-v2".to_string()
}

fn default_dimension() -> usize {
    384
}

fn default_cache_dir() -> PathBuf {
    default_memory_dir().join("models")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_limit")]
    pub default_limit: usize,

    #[serde(default = "max_limit")]
    pub max_limit: usize,

    #[serde(default = "default_threshold")]
    pub similarity_threshold: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_limit: default_limit(),
            max_limit: max_limit(),
            similarity_threshold: default_threshold(),
        }
    }
}

fn default_limit() -> usize {
    10
}

fn max_limit() -> usize {
    100
}

fn default_threshold() -> f32 {
    0.7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default = "true_fn")]
    pub enabled: bool,

    #[serde(default = "true_fn")]
    pub stdio: bool,

    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stdio: true,
            timeout_seconds: default_timeout(),
        }
    }
}

fn true_fn() -> bool {
    true
}

fn default_timeout() -> u64 {
    30
}

impl Config {
    /// Load configuration from file
    pub fn load(path: &Path) -> Result<Self, anyhow::Error> {
        if !path.exists() {
            tracing::info!("Config file not found at {:?}, using defaults", path);
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;

        // Validate paths are absolute
        let memory_dir = if config.memory_dir.is_absolute() {
            config.memory_dir
        } else {
            std::env::current_dir()?.join(&config.memory_dir)
        };

        let db_path = if config.db_path.is_absolute() {
            config.db_path
        } else {
            std::env::current_dir()?.join(&config.db_path)
        };

        Ok(Self {
            memory_dir,
            db_path,
            ..config
        })
    }

    /// Save configuration to file
    pub fn save(&self, path: &Path) -> Result<(), anyhow::Error> {
        let content = serde_yaml::to_string(self)?;
        std::fs::create_dir_all(path.parent().expect("Config path must have parent"))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Initialize config file at default location if it doesn't exist
    pub fn initialize_default() -> Result<PathBuf, anyhow::Error> {
        let default_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".total-recall")
            .join("config.yaml");

        if !default_path.exists() {
            let config = Config::default();
            config.save(&default_path)?;
            tracing::info!("Created default config at {}", default_path.display());
        }

        Ok(default_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_paths() {
        let config = Config::default();

        assert!(config.memory_dir.ends_with(".total-recall"));
        assert!(config.db_path.ends_with(".total-recall/memory.db"));
    }

    #[test]
    fn test_embedding_model() {
        let config = Config::default();
        assert_eq!(
            config.embedding.model,
            "sentence-transformers/all-MiniLM-L6-v2"
        );
        assert_eq!(config.embedding.dimension, 384);
    }

    #[test]
    fn test_search_limit() {
        let config = Config::default();
        assert_eq!(config.search.default_limit, 10);
        assert_eq!(config.search.max_limit, 100);
    }

    #[test]
    fn test_config_deserialization() {
        let yaml = r#"
memory_dir: /custom/memory
db_path: /custom/memory.db
logging:
  level: debug
search:
  default_limit: 20
"#;

        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.memory_dir, PathBuf::from("/custom/memory"));
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.search.default_limit, 20);
    }

    #[test]
    fn test_config_save_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.yaml");

        let config = Config::default();
        config.save(&config_path).unwrap();

        let loaded = Config::load(&config_path).unwrap();
        assert_eq!(loaded.memory_dir.display().to_string(), ".");

        // Paths are converted to absolute on load
        assert!(loaded.db_path.is_absolute());
    }
}
