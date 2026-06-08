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
        "Snowflake/snowflake-arctic-embed-l-v2.0"
    );
    assert_eq!(
        config.embedding.model_path,
        PathBuf::from("/data/models/embed")
    );
    assert_eq!(config.embedding.dimension, 1024);
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
    assert!(loaded.memory_dir.is_absolute());
    assert!(loaded.memory_dir.ends_with(".total-recall"));

    // Paths are converted to absolute on load
    assert!(loaded.db_path.is_absolute());
}
