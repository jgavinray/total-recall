# Total-Recall Implementation Guide

**Build this file-based agentic memory MCP server with Rust**

Follow these steps **in order**. Each step has acceptance criteria - DO NOT proceed until ALL criteria are met and tests pass.

---

## Prerequisites Checklist

Before starting, ensure you have:

- [ ] Rust 1.85+ installed (`rustc --version`, `cargo --version`)
- [ ] SQLite 3.40+ with VSS extension (`sqlite3 --version`)
- [ ] Basic understanding of: Rust async, MCP protocol, SQLite
- [ ] Testing knowledge: `cargo test`, assertions, mocking basics

---

## Step 0: Project Initialization

**Goal:** Create the basic project structure with build script for sqlite-vec

**Commands to run:**

```bash
cd /Users/jgavinray/dev/memory
mkdir -p total-recall/src/{memory,mcp/tools,config}
cd total-recall
cargo init --name total-recall
```

**Files to create:**

1. `Cargo.toml` with dependencies:

```toml
[package]
name = "total-recall"
version = "0.1.0"
edition = "2024"
authors = ["J. Gavin Ray"]
build = "build.rs"

[dependencies]
# MCP protocol
mcp = "0.12"

# Async runtime
tokio = { version = "1.46", features = ["full", "rt-multi-thread", "macros"] }

# SQLite with bundled version (we'll load sqlite-vec extension)
rusqlite = { version = "0.34", features = ["bundled", "vss"] }

# ONNX for embeddings
ort = "2.0"
ort-extras = "2.0"

# Async pool for SQLite
deadpool = { version = "0.12", features = ["rt_tokio_1"] }
deadpool-sqlite = "0.11"

# YAML config parsing
serde = { version = "1.0", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1.0"

# UUID generation
uuid = { version = "1.17", features = ["v4", "serde"] }

# Date/time handling
chrono = { version = "0.4", features = ["serde"] }

# Tracing/logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# CLI parsing
clap = { version = "4.5", features = ["derive"] }

# Vector math
ndarray = "0.16"

# Async trait for MCP
async-trait = "0.1"

[dev-dependencies]
tempfile = "3.21"

[build-dependencies]
# Build script dependencies
cc = "1.2"
```

2. `build.rs` for sqlite-vec compilation:

```rust
fn main() {
    // Download sqlite-vec source if not present
    let sqlite_vec_dir = "vendor/sqlite-vec";
    
    if !std::path::Path::new(sqlite_vec_dir).exists() {
        std::fs::create_dir_all(sqlite_vec_dir).expect("Failed to create sqlite-vec dir");
        
        // Download sqlite-vec source code
        let resp = ureq::get("https://api.github.com/repos/asg017/sqlite-vec/tarball/main")
            .call()
            .expect("Failed to download sqlite-vec");
        
        let tar_bytes = resp.into_bytes().expect("Failed to read tarball");
        use flate2::read::GzDecoder;
        use tar::Archive;
        
        let decoder = GzDecoder::new(std::io::Cursor::new(tar_bytes));
        let mut archive = Archive::new(decoder);
        archive.unpack(sqlite_vec_dir).expect("Failed to extract sqlite-vec");
    }
    
    // Build sqlite-vec as C extension
    cc::Build::new()
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .include(&format!("{}/{}", sqlite_vec_dir, "sqlite-vec-*")) // wildcard expansion needed
        .file("vendor/sqlite-vec/sqlite-vec-*/src/api.c")
        .compile("sqlite-vec");
    
    println!("cargo:rerun-if-changed=build.rs");
}
```

**Acceptance Criteria:**
```bash
cd /Users/jgavinray/dev/memory/total-recall
cargo check --release
# Must return 0 errors
```

**Verification:**
```bash
cd /Users/jgavinray/dev/memory/total-recall
cargo build --release 2>&1
# Must complete without errors
```

---

## Step 1: Error Handling Module

**Goal:** Create centralized error types

**File:** `src/error.rs`

```rust
use thiserror::Error;
use std::path::PathBuf;

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
}

pub type Result<T> = std::result::Result<T, MemoryError>;
```

**Create test file:** `tests/error_tests.rs`

```rust
use total_recall::error::{MemoryError, Result};

#[test]
fn test_memory_error_database_variant() {
    let err = MemoryError::Database(rusqlite::Error::QueryReturnedNoRows);
    assert!(matches!(err, MemoryError::Database(_)));
}

#[test]
fn test_memory_error_file_not_found() {
    let path = std::path::PathBuf::from(".total-recall/test.md");
    let err = MemoryError::FileNotFoundError(path);
    assert!(matches!(err, MemoryError::FileNotFoundError(_)));
    assert!(err.to_string().contains("test.md"));
}

#[test]
fn test_memory_error_file_exists() {
    let path = std::path::PathBuf::from("03/06/2026.md");
    let err = MemoryError::FileExistsError(path);
    assert!(matches!(err, MemoryError::FileExistsError(_)));
}

#[test]
fn test_result_type() {
    let result: Result<String> = Ok("test".to_string());
    assert!(result.is_ok());
    
    let result: Result<String> = Err(MemoryError::NotFound("test".to_string()));
    assert!(result.is_err());
}
```

**Run tests:**
```bash
cargo test --test error_tests
# All tests must pass
```

---

## Step 2: Data Models

**Goal:** Define Note, Observation, and Metadata structures

**File:** `src/memory/models.rs`

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Metadata extracted from frontmatter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMetadata {
    pub title: Option<String>,
    pub date: Option<String>,      // mm/dd/yyyy
    pub r#type: Option<String>,    // Note type (meeting, decision, etc.)
    pub tags: Option<Vec<String>>, // Tags from frontmatter
    pub archived: Option<bool>,    // Soft delete flag
}

/// An observation chunk (semantic unit for embedding)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub note_id: String,           // links to note date mm/dd/yyyy
    pub timestamp: String,         // e.g., "14:30"
    pub section: Option<String>,   // e.g., "## Discussion"
    pub category: Option<String>,  // decision, action, note, idea, question, risk
    pub content: String,
    pub full_context: String,      // surrounding markdown
    pub tags: Vec<String>,         // tags from observation
}

/// A complete note (daily entry)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub date: String,              // mm/dd/yyyy
    pub metadata: NoteMetadata,
    pub content: String,           // full markdown content
    pub observations: Vec<Observation>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
}

impl NoteMetadata {
    pub fn parse_frontmatter(text: &str) -> Result<Self> {
        // Extract YAML frontmatter between --- markers
        let start = text.find("---");
        let end = text[start.unwrap_or(0)..].find("---\n").map(|i| start.unwrap_or(0) + i);
        
        if let (Some(_start), Some(end)) = (start, end) {
            let frontmatter = &text[start + 3..start + end - 3];
            serde_yaml::from_str(frontmatter)
                .map_err(|e| MemoryError::ParseError(format!("failed to parse YAML: {}", e)))
        } else {
            Ok(NoteMetadata {
                title: None,
                date: None,
                r#type: None,
                tags: None,
                archived: None,
            })
        }
    }
}
```

**Create test file:** `tests/models_tests.rs`

```rust
use total_recall::memory::models::{NoteMetadata, Observation};

#[test]
fn test_parse_frontmatter() {
    let markdown = r#"---
title: "Test Note"
date: 03/06/2026
type: meeting
tags:
  - test
---

This is the content"#;

    let metadata = NoteMetadata::parse_frontmatter(markdown).unwrap();
    
    assert_eq!(metadata.title, Some("Test Note".to_string()));
    assert_eq!(metadata.date, Some("03/06/2026".to_string()));
    assert_eq!(metadata.r#type, Some("meeting".to_string()));
    assert_eq!(metadata.tags, Some(vec!["test".to_string()]));
}

#[test]
fn test_parse_frontmatter_empty() {
    let markdown = "No frontmatter here";
    let metadata = NoteMetadata::parse_frontmatter(markdown).unwrap();
    
    assert_eq!(metadata.title, None);
    assert_eq!(metadata.date, None);
}

#[test]
fn test_observation_structure() {
    let obs = Observation {
        id: Uuid::new_v4().to_string(),
        note_id: "03/06/2026".to_string(),
        timestamp: "14:30".to_string(),
        section: Some("## Discussion".to_string()),
        category: Some("decision".to_string()),
        content: "Adopt Rust".to_string(),
        full_context: "## Discussion\n- [decision] Adopt Rust".to_string(),
        tags: vec!["rust".to_string()],
    };
    
    assert_eq!(obs.note_id, "03/06/2026");
    assert_eq!(obs.category, Some("decision".to_string()));
}

#[test]
fn test_parse_frontmatter_partial() {
    let markdown = r#"---
title: "Partial"
---

Content"#;

    let metadata = NoteMetadata::parse_frontmatter(markdown).unwrap();
    
    assert_eq!(metadata.title, Some("Partial".to_string()));
    assert_eq!(metadata.date, None); // Not provided
}
```

**Run tests:**
```bash
cargo test --test models_tests
# All tests must pass
```

---

## Step 3: File Parser Module

**Goal:** Parse observations from markdown content

**File:** `src/memory/file_parser.rs`

```rust
use crate::error::{MemoryError, Result};
use crate::memory::models::Observation;
use std::collections::HashMap;

pub struct FileParser;

impl FileParser {
    pub fn parse_observations(content: &str) -> Result<Vec<Observation>> {
        let mut observations = Vec::new();
        let mut current_section = None;
        let mut current_timestamp = String::new();
        
        for (line_num, line) in content.lines().enumerate() {
            // Check for section headers
            if line.starts_with("## ") {
                current_section = Some(line[3..].to_string());
                continue;
            }
            
            // Check for timestamp header (e.g., "## 14:30")
            if line.starts_with("## ") {
                let rest = &line[3..];
                if rest.chars().take(2).all(|c| c.is_ascii_digit()) && 
                   rest.chars().nth(2) == Some(':') {
                    current_timestamp = rest.to_string();
                    continue;
                }
            }
            
            // Skip frontmatter and other lines
            if line.starts_with("---") {
                continue;
            }
            
            // Check for observation pattern: "- [category] content"
            if line.trim().starts_with("- [") {
                if let Some(obs) = Self::parse_observation_line(line, &current_timestamp, &current_section, line_num + 1) {
                    observations.push(obs);
                }
            }
        }
        
        Ok(observations)
    }
    
    fn parse_observation_line(line: &str, timestamp: &str, section: &Option<String>, line_num: usize) -> Option<Observation> {
        let trimmed = line.trim_start().trim_start_matches("- ");
        
        // Must start with [category]
        if !trimmed.starts_with('[') {
            return None;
        }
        
        // Find closing bracket
        let end_bracket = trimmed.find(']')?;
        let category = trimmed[1..end_bracket].to_string();
        
        // Rest is the content
        let mut content = trimmed[end_bracket + 2..].to_string();
        
        // Extract tags (#tag)
        let mut tags = Vec::new();
        let mut full_context = line.to_string();
        
        // Clean line for context
        full_context.trim().to_string();
        
        // Extract hashtags
        for part in content.split('#') {
            let part = part.trim();
            if part.starts_with(|c: char| c.is_alphanumeric()) {
                // This is a tag (not prefix of word)
                let tag = part.trim_start_matches(|c: char| c.is_alphanumeric()).trim().trim_start_matches('#');
                if !tag.is_empty() && tag.chars().all(|c| c.is_alphanumeric() || c == '-') {
                    tags.push(tag.to_string());
                    // Remove the tag from content
                    content = content.split('#').next().unwrap_or("").trim().to_string();
                }
            }
        }
        
        Some(Observation {
            id: uuid::Uuid::new_v4().to_string(),
            note_id: String::new(), // Will be set later
            timestamp: timestamp.to_string(),
            section: section.clone(),
            category: Some(category),
            content,
            full_context,
            tags,
        })
    }
}
```

**Create test file:** `tests/parser_tests.rs`

```rust
use total_recall::memory::file_parser::FileParser;

#[test]
fn test_parse_single_observation() {
    let content = r#"## Discussion

- [decision] Adopt Rust for microservice

More text"#;

    let observations = FileParser::parse_observations(content).unwrap();
    
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].category, Some("decision".to_string()));
    assert_eq!(observations[0].content, "Adopt Rust for microservice");
}

#[test]
fn test_parse_multiple_observations() {
    let content = r#"## 14:30

- [decision] Decision 1
- [action] Action 1 item

## 14:45

- [note] Note about something"#;

    let observations = FileParser::parse_observations(content).unwrap();
    
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0].category, Some("decision".to_string()));
    assert_eq!(observations[1].category, Some("action".to_string()));
    assert_eq!(observations[2].category, Some("note".to_string()));
}

#[test]
fn test_parse_with_tags() {
    let content = r#"- [decision] Adopt Rust #architecture

- [action] Task #urgent"#;

    let observations = FileParser::parse_observations(content).unwrap();
    
    assert_eq!(observations[0].tags, vec!["architecture".to_string()]);
    assert_eq!(observations[1].tags, vec!["urgent".to_string()]);
}

#[test]
fn test_parse_no_timestamp() {
    let content = r#"No timestamp section

- [decision] Just a decision"#;

    let observations = FileParser::parse_observations(content).unwrap();
    
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].timestamp, String::new());
}

#[test]
fn test_parse_empty_content() {
    let content = "Just text, no observations";
    let observations = FileParser::parse_observations(content).unwrap();
    
    assert!(observations.is_empty());
}
```

**Run tests:**
```bash
cargo test --test parser_tests
# All tests must pass
```

---

## Step 4: SQLite Store with VSS

**Goal:** Create SQLite database with vector support

**File:** `src/memory/store.rs`

```rust
use crate::error::{MemoryError, Result};
use crate::memory::models::{Note, Observation};
use chrono::Utc;
use rusqlite::{Connection, params};
use std::path::Path;
use std::sync::Arc;

pub struct MemoryStore {
    connection: Arc<Connection>,
}

impl MemoryStore {
    pub fn new(db_path: &Path) -> Result<Self> {
        // Ensure directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Connect to database
        let conn = Connection::open(db_path)?;
        
        // Enable WAL mode for better concurrent access
        conn.execute("PRAGMA journal_mode=WAL", [])?;
        
        // Create tables
        conn.execute_batch(
            "
            -- Notes table
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                date TEXT NOT NULL UNIQUE,
                title TEXT,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                archived INTEGER DEFAULT 0
            );

            -- Observations table (chunks for vector search)
            CREATE TABLE IF NOT EXISTS observations (
                id TEXT PRIMARY KEY,
                note_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                section TEXT,
                category TEXT,
                content TEXT NOT NULL,
                context TEXT NOT NULL,
                tags TEXT,
                FOREIGN KEY (note_id) REFERENCES notes(date) ON DELETE CASCADE
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_observations_note_id ON observations(note_id);
            CREATE INDEX IF NOT EXISTS idx_observations_category ON observations(category);
            CREATE INDEX IF NOT EXISTS idx_notes_date ON notes(date DESC);
            "
        )?;
        
        // Enable SQLite VSS extension if available
        // Note: This assumes rusqlite is built with VSS support
        if let Err(e) = conn.execute("SELECT sqlite3_vss_init(db)", []) {
            tracing::warn!("VSS extension may not be available: {}", e);
        }
        
        Ok(Self {
            connection: Arc::new(conn),
        })
    }
    
    /// Create a new note (immutably - does not overwrite)
    pub fn create_note(&self, date: &str, content: &str) -> Result<Note> {
        // Check if note already exists
        let exists: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM notes WHERE date = ?1",
            [date],
            |row| row.get(0),
        )?;
        
        if exists > 0 {
            return Err(MemoryError::FileExistsError(
                format!("{}.md", date).into()
            ));
        }
        
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();
        
        // Insert into notes table
        self.connection.execute(
            "INSERT INTO notes (id, date, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, date, content, now, now]
        )?;
        
        // Parse and insert observations
        let observations = self.parse_and_insert_observations(&id, date, content)?;
        
        // Read back the note
        let note = self.read_note(date)?;
        
        Ok(note)
    }
    
    fn parse_and_insert_observations(&self, note_id: &str, date: &str, content: &str) -> Result<Vec<Observation>> {
        // This will call out to file_parser module
        use crate::memory::file_parser::FileParser;
        let observations = FileParser::parse_observations(content)?;
        
        let mut inserted = Vec::new();
        for mut obs in observations {
            obs.note_id = date.to_string();
            let obs_id = uuid::Uuid::new_v4().to_string();
            
            self.connection.execute(
                "INSERT INTO observations (id, note_id, timestamp, section, category, content, context, tags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    obs_id,
                    obs.note_id,
                    obs.timestamp,
                    obs.section,
                    obs.category,
                    obs.content,
                    obs.full_context,
                    obs.tags.as_slice()
                ],
            )?;
            
            inserted.push(obs);
        }
        
        Ok(inserted)
    }
    
    /// Read a note by date (mm/dd/yyyy)
    pub fn read_note(&self, date: &str) -> Result<Note> {
        let row = self.connection.query_row(
            "SELECT id, date, title, content, created_at, updated_at, archived 
             FROM notes WHERE date = ?1",
            [date],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?
            )),
        )?;
        
        let (id, date, title, content, created_at, updated_at, archived) = row;
        
        let observations = self.get_observations_for_note(date)?;
        
        Ok(Note {
            id,
            date,
            metadata: crate::memory::models::NoteMetadata {
                title,
                date: Some(date.clone()),
                r#type: None,
                tags: None,
                archived: Some(archived > 0),
            },
            content,
            observations,
            created_at,
            updated_at,
            archived: archived > 0,
        })
    }
    
    fn get_observations_for_note(&self, date: &str) -> Result<Vec<Observation>> {
        let mut stmt = self.connection.prepare(
            "SELECT id, note_id, timestamp, section, category, content, context, tags
             FROM observations WHERE note_id = ?1",
        )?;
        
        let obs_rows = stmt.query_map([date], |row| {
            Ok(Observation {
                id: row.get(0)?,
                note_id: row.get(1)?,
                timestamp: row.get(2)?,
                section: row.get::<_, Option<String>>(3)?,
                category: row.get::<_, Option<String>>(4)?,
                content: row.get(5)?,
                full_context: row.get(6)?,
                tags: row.get::<_, Vec<String>>(7)?,
            })
        })?;
        
        obs_rows.collect::<Result<Vec<_>, _>>()
    }
    
    /// Archive a note (soft delete)
    pub fn archive_note(&self, date: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE notes SET archived = 1 WHERE date = ?1",
            [date],
        )?;
        Ok(())
    }
    
    /// Restore an archived note
    pub fn restore_note(&self, date: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE notes SET archived = 0 WHERE date = ?1",
            [date],
        )?;
        Ok(())
    }
    
    /// Get recent notes (archived excluded by default)
    pub fn get_recent_notes(&self, limit: usize, days: usize, include_archived: bool) -> Result<Vec<Note>> {
        let days_ago = Utc::now().timestamp() - (days as i64 * 86400);
        
        let query = if include_archived {
            "SELECT id, date, title, content, created_at, updated_at, archived 
             FROM notes 
             WHERE updated_at >= ?1
             ORDER BY updated_at DESC
             LIMIT ?2"
        } else {
            "SELECT id, date, title, content, created_at, updated_at, archived 
             FROM notes 
             WHERE updated_at >= ?1 AND archived = 0
             ORDER BY updated_at DESC
             LIMIT ?2"
        };
        
        let mut stmt = self.connection.prepare(query)?;
        
        let notes = stmt.query_map([days_ago, limit as i64], |row| {
            let id = row.get(0)?;
            let date = row.get(1)?;
            let title = row.get::<_, Option<String>>(2)?;
            let content = row.get(3)?;
            let created_at = row.get(4)?;
            let updated_at = row.get(5)?;
            let archived = row.get::<_, i64>(6)? > 0;
            
            Ok(Note {
                id,
                date,
                metadata: crate::memory::models::NoteMetadata {
                    title,
                    date: Some(date.clone()),
                    r#type: None,
                    tags: None,
                    archived: Some(archived),
                },
                content,
                observations: Vec::new(), // Will be populated separately
                created_at,
                updated_at,
                archived,
            })
        })?;
        
        notes.collect::<Result<Vec<_>, _>>()
    }
    
    /// Search notes using vector similarity (via VSS)
    pub fn search_notes(&self, query_embedding: &[f32; 384], limit: usize, include_archived: bool) -> Result<Vec<Note>> {
        // Use VSS vector similarity search
        let query = if include_archived {
            "SELECT n.id, n.date, n.title, n.content, n.updated_at, n.archived
             FROM notes n
             JOIN observations o ON n.date = o.note_id
             WHERE o.embedding MATCH ?1 AND k = ?2
             GROUP BY n.date
             ORDER BY AVG(o.distance)
             LIMIT ?3"
        } else {
            "SELECT n.id, n.date, n.title, n.content, n.updated_at, n.archived
             FROM notes n
             JOIN observations o ON n.date = o.note_id
             WHERE o.embedding MATCH ?1 AND k = ?2 AND n.archived = 0
             GROUP BY n.date
             ORDER BY AVG(o.distance)
             LIMIT ?3"
        };
        
        let mut stmt = self.connection.prepare(query)?;
        
        let notes = stmt.query_map([query_embedding, limit as i64], |row| {
            let id = row.get(0)?;
            let date = row.get(1)?;
            let title = row.get::<_, Option<String>>(2)?;
            let content = row.get(3)?;
            let updated_at = row.get(4)?;
            let archived = row.get::<_, i64>(5)? > 0;
            
            Ok(Note {
                id,
                date,
                metadata: crate::memory::models::NoteMetadata {
                    title,
                    date: Some(date.clone()),
                    r#type: None,
                    tags: None,
                    archived: Some(archived),
                },
                content,
                observations: Vec::new(),
                created_at: updated_at,
                updated_at,
                archived,
            })
        })?;
        
        notes.collect::<Result<Vec<_>, _>>()
    }
}
```

**Create test file:** `tests/store_tests.rs`

```rust
use total_recall::memory::store::MemoryStore;
use tempfile::TempDir;

fn create_test_db() -> (MemoryStore, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let store = MemoryStore::new(&db_path).unwrap();
    (store, temp_dir)
}

#[test]
fn test_create_note() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## 14:30
- [decision] Test decision
- [note] Test note"#;

    let note = store.create_note("03/06/2026", content).unwrap();
    
    assert_eq!(note.date, "03/06/2026");
    assert!(note.content.contains("Test decision"));
}

#[test]
fn test_create_note_already_exists() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## Test
- [note] Content"#;

    // First creation should succeed
    store.create_note("03/07/2026", content).unwrap();
    
    // Second creation should fail
    let result = store.create_note("03/07/2026", content);
    assert!(result.is_err());
}

#[test]
fn test_read_note() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## Discussion
- [decision] Read test"#;

    store.create_note("03/08/2026", content).unwrap();
    
    let note = store.read_note("03/08/2026").unwrap();
    
    assert_eq!(note.date, "03/08/2026");
    assert!(note.content.contains("Read test"));
    assert!(!note.archived);
}

#[test]
fn test_archive_note() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## Test
- [note] Before archive"#;

    store.create_note("03/09/2026", content).unwrap();
    
    // Not archived initially
    let note_before = store.read_note("03/09/2026").unwrap();
    assert!(!note_before.archived);
    
    // Archive it
    store.archive_note("03/09/2026").unwrap();
    
    // Now archived
    let note_after = store.read_note("03/09/2026").unwrap();
    assert!(note_after.archived);
}

#[test]
fn test_restore_note() {
    let (store, _temp) = create_test_db();
    
    let content = r#"## Test"#;
    store.create_note("03/10/2026", content).unwrap();
    store.archive_note("03/10/2026").unwrap();
    
    // Restore
    store.restore_note("03/10/2026").unwrap();
    
    let note = store.read_note("03/10/2026").unwrap();
    assert!(!note.archived);
}

#[test]
fn test_get_recent_notes() {
    let (store, _temp) = create_test_db();
    
    // Create notes
    store.create_note("03/06/2026", "# Test").unwrap();
    store.create_note("03/07/2026", "# Test").unwrap();
    store.create_note("03/08/2026", "# Test").unwrap();
    
    // Get recent (last 10 days, should get all 3)
    let recent = store.get_recent_notes(10, 10, false).unwrap();
    assert_eq!(recent.len(), 3);
}

#[test]
fn test_get_recent_notes_excludes_archived() {
    let (store, _temp) = create_test_db();
    
    store.create_note("03/06/2026", "# Test").unwrap();
    store.archive_note("03/06/2026").unwrap();
    store.create_note("03/07/2026", "# Test").unwrap();
    
    // Should only return non-archived
    let recent = store.get_recent_notes(10, 10, false).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].date, "03/07/2026");
}

#[test]
fn test_get_recent_notes_includes_archived_when_requested() {
    let (store, _temp) = create_test_db();
    
    store.create_note("03/06/2026", "# Test").unwrap();
    store.archive_note("03/06/2026").unwrap();
    
    // Should return archived when include_archived=true
    let recent = store.get_recent_notes(10, 10, true).unwrap();
    assert_eq!(recent.len(), 1);
}
```

**Run tests:**
```bash
cargo test --test store_tests
# Must run all tests successfully
```

---

## Step 5: Embedding System

**Goal:** Implement ONNX-based text embeddings (384-dim MiniLM)

**File:** `src/memory/embedder.rs`

```rust
use crate::error::{MemoryError, Result};
use ndarray::{Array, Array1};

pub struct Embedder {
    // Placeholder - will load actual ONNX model in implementation
    // For now, return deterministic pseudo-random embeddings
}

impl Embedder {
    pub fn new() -> Result<Self> {
        tracing::info!("Initializing ONNX embedder (all-MiniLM-L6-v2)");
        
        // TODO: Download and load model weights
        // Expected model: sentence-transformers/all-MiniLM-L6-v2
        // Location: ~/.total-recall/models/sentence-transformers/all-MiniLM-L6-v2.onnx
        
        // For initial implementation, use simple hash-based embedding
        // This can be replaced with actual ONNX model later
        
        Ok(Self)
    }
    
    pub fn embed(&self, text: &str) -> [f32; 384] {
        // Simple hash-based embedding for initial implementation
        // Replace with actual ONNX inference when model is loaded
        
        let mut embedding = [0.0f32; 384];
        
        // XOR-based hash spreading
        let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        for byte in text.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        
        // Distribute hash across 384 dimensions
        for i in 0..384 {
            let offset = (hash >> (i % 64)) as usize;
            embedding[i] = ((offset as i64 % 1000) as f32 - 500.0) / 500.0;
            
            // Add some variation based on text content
            if i % 2 == 0 && byte_count_in_slice(text.as_bytes(), i) > 0 {
                embedding[i] *= 1.1;
            }
        }
        
        // Normalize to roughly unit length
        let norm = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.001 {
            for e in &mut embedding {
                *e /= norm;
            }
        }
        
        embedding
    }
    
    pub fn embed_batch(&self, texts: &[&str]) -> Vec<[f32; 384]> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

// Helper to count bytes in a slice (simplified)
fn byte_count_in_slice(bytes: &[u8], idx: usize) -> usize {
    bytes.get(idx).map_or(0, |_| 1)
}
```

**Create test file:** `tests/embedder_tests.rs`

```rust
use total_recall::memory::embedder::Embedder;

fn create_embedder() -> Embedder {
    Embedder::new().unwrap()
}

#[test]
fn test_embed_deterministic() {
    let embedder = create_embedder();
    
    // Same input should produce same output
    let embedding1 = embedder.embed("hello");
    let embedding2 = embedder.embed("hello");
    
    assert_eq!(embedding1, embedding2);
}

#[test]
fn test_embed_different_inputs() {
    let embedder = create_embedder();
    
    let embedding1 = embedder.embed("hello");
    let embedding2 = embedder.embed("world");
    
    // Different inputs should produce different outputs
    assert_ne!(embedding1, embedding2);
}

#[test]
fn test_embed_batch() {
    let embedder = create_embedder();
    
    let texts = vec!["hello", "world", "test"];
    let embeddings = embedder.embed_batch(&texts);
    
    assert_eq!(embeddings.len(), 3);
    
    // All should be different
    assert_ne!(embeddings[0], embeddings[1]);
    assert_ne!(embeddings[1], embeddings[2]);
}

#[test]
fn test_embed_empty_string() {
    let embedder = create_embedder();
    
    let embedding = embedder.embed("");
    
    // Should not crash, produces some output
    let non_zero_count = embedding.iter().filter(|&&x| x != 0.0).count();
    assert!(non_zero_count > 0);
}

#[test]
fn test_embed_normalization() {
    let embedder = create_embedder();
    
    let embedding = embedder.embed("test text");
    
    // Check approximate normalization
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(norm > 0.5 && norm < 1.5); // Roughly unit length
}
```

**Run tests:**
```bash
cargo test --test embedder_tests
# All tests must pass
```

---

## Step 6: MCP Server Implementation

**Goal:** Create MCP server with all 5 tools

**File:** `src/mcp/server.rs`

```rust
use crate::error::Result;
use crate::memory::store::MemoryStore;
use crate::memory::models::NoteMetadata;
use mcp::server::{Server, ServerHandler};
use mcp::tools::{ListToolsResult, CallToolResult, Tool};
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct MemoryMcpServer {
    store: Arc<RwLock<MemoryStore>>,
    memory_dir: std::path::PathBuf,
}

impl MemoryMcpServer {
    pub fn new(store: MemoryStore, memory_dir: std::path::PathBuf) -> Self {
        Self {
            store: Arc::new(RwLock::new(store)),
            memory_dir,
        }
    }
}

#[allow(dead_code)]
impl MemoryMcpServer {
    pub async fn list_tools(&self) -> ListToolsResult {
        let tools = vec![
            Tool {
                name: "write_note".to_string(),
                description: "Create a new memory note. Returns error if date already exists."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "content": {
                            "type": "string",
                            "description": "The note content (markdown format)"
                        },
                        "timestamp": {
                            "type": "string",
                            "description": "Optional timestamp section header (e.g., '14:30')",
                            "default": null
                        }
                    },
                    "required": ["content"]
                }),
            },
            Tool {
                name: "read_note".to_string(),
                description: "Read a note by date in mm/dd/yyyy format".to_string(),
                input_schema: serde_json::json!({
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
                description: "Search notes using semantic vector search".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (will be embedded)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results",
                            "default": 10
                        },
                        "include_archived": {
                            "type": "boolean",
                            "default": false
                        }
                    },
                    "required": ["query"]
                }),
            },
            Tool {
                name: "recent_notes".to_string(),
                description: "Get notes from the last N days".to_string(),
                input_schema: serde_json::json!({
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
                description: "Get all observations from a specific date".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "date": {
                            "type": "string",
                            "description": "Date in mm/dd/yyyy format"
                        },
                        "category_filter": {
                            "type": "string",
                            "description": "Optional: filter by category"
                        }
                    },
                    "required": ["date"]
                }),
            },
        ];
        
        ListToolsResult { tools }
    }
}

// Implement MCP server trait
#[async_trait::async_trait]
impl ServerHandler for MemoryMcpServer {
    type Error = crate::error::MemoryError;
    type ToolExecutor = Server;
    
    async fn initialize(&self, _client_version: &str) -> mcp::protocol::InitializeResult {
        mcp::protocol::InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            server_info: mcp::protocol::Implementation {
                name: "total-recall".to_string(),
                version: "0.1.0".to_string(),
            },
            capabilities: mcp::protocol::ServerCapabilities {
                tools: Some(mcp::protocol::ToolsCapability::ListChanged),
                ..Default::default()
            },
            instructions: Some("Agentic memory MCP server. Use write_note, read_note, search_notes, recent_notes, build_context".to_string()),
        }
    }
    
    async fn list_tools(&self) -> ListToolsResult {
        self.list_tools().await
    }
    
    async fn call_tool(&self, _name: &str, _arguments: serde_json::Value) -> Result<CallToolResult, Self::Error> {
        // This is where tool implementations go
        // For now, return not implemented
        unimplemented!("Tool implementation in next step")
    }
}
```

**Create test file:** `tests/mcp_server_tests.rs`

```rust
use crate::memory::store::MemoryStore;
use std::path::PathBuf;

// Note: Full MCP server testing requires mocking the MCP protocol
// For now, we test the server construction and tool listing

#[test]
fn test_server_creation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let memory_dir = temp_dir.path().join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    
    let store = MemoryStore::new(&db_path).unwrap();
    let _server = total_recall::mcp::server::MemoryMcpServer::new(
        store, 
        memory_dir
    );
    
    // If we get here, server creation succeeded
}

#[async_std::test]
async fn test_list_tools() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let memory_dir = temp_dir.path().join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    
    let store = MemoryStore::new(&db_path).unwrap();
    let server = total_recall::mcp::server::MemoryMcpServer::new(
        store, 
        memory_dir
    );
    
    let tools_response = server.list_tools().await;
    
    assert_eq!(tools_response.tools.len(), 5);
    
    let tool_names: Vec<&str> = tools_response.tools.iter()
        .map(|t| t.name.as_str())
        .collect();
    
    assert!(tool_names.contains(&"write_note"));
    assert!(tool_names.contains(&"read_note"));
    assert!(tool_names.contains(&"search_notes"));
    assert!(tool_names.contains(&"recent_notes"));
    assert!(tool_names.contains(&"build_context"));
}

#[async_std::test]
async fn test_tool_descriptions() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test.db");
    let memory_dir = temp_dir.path().join("memory");
    std::fs::create_dir_all(&memory_dir).unwrap();
    
    let store = MemoryStore::new(&db_path).unwrap();
    let server = total_recall::mcp::server::MemoryMcpServer::new(
        store, 
        memory_dir
    );
    
    let tools_response = server.list_tools().await;
    
    let write_note = tools_response.tools.iter()
        .find(|t| t.name == "write_note")
        .unwrap();
    
    assert!(write_note.description.contains("Create") || 
            write_note.description.contains("note"));
}
```

**Run tests:**
```bash
cargo test --test mcp_server_tests
```

---

## Step 7: MCP Tool Implementations

**Goal:** Implement the actual tool logic

**File:** `src/mcp/tools.rs`

```rust
use crate::error::Result;
use crate::memory::embedder::Embedder;
use crate::memory::store::MemoryStore;
use async_trait::async_trait;
use mcp::tools::{CallToolError, CallToolResult, ToolExecutor};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ToolExecutorImpl {
    store: Arc<RwLock<MemoryStore>>,
    memory_dir: std::path::PathBuf,
    embedder: Arc<Embedder>,
}

impl ToolExecutorImpl {
    pub fn new(store: MemoryStore, memory_dir: std::path::PathBuf) -> Self {
        let embedder = Embedder::new().unwrap();
        Self {
            store: Arc::new(RwLock::new(store)),
            memory_dir,
            embedder: Arc::new(embedder),
        }
    }
    
    pub async fn write_note(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(arguments)
            .map_err(|e| CallToolError::ParseError(e.to_string()))?;
        
        let content = args.get("content")
            .ok_or_else(|| CallToolError::ParseError("missing required field: content".to_string()))?
            .as_str()
            .ok_or_else(|| CallToolError::ParseError("content must be string".to_string()))?;
        
        let timestamp = args.get("timestamp")
            .and_then(|v| v.as_str());
        
        // Get current date
        let current_date = chrono::Utc::now().format("%m/%d/%Y").to_string();
        
        // Add timestamp header if provided
        let content = if let Some(ts) = timestamp {
            format!("## {}\n\n{}", ts, content)
        } else {
            format!("\n{}", content)
        };
        
        let store = self.store.write().await;
        
        match store.create_note(&current_date, &content) {
            Ok(_) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Successfully created note for {}", current_date),
                }],
                is_error: Some(false),
            }),
            Err(crate::error::MemoryError::FileExistsError(_)) => {
                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text {
                        text: format!("Note for {} already exists. Use append_note to add content.", current_date),
                    }],
                    is_error: Some(true),
                })
            },
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error creating note: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }
    
    pub async fn read_note(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(arguments)
            .map_err(|e| CallToolError::ParseError(e.to_string()))?;
        
        let date = args.get("date")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CallToolError::ParseError("missing required field: date".to_string()))?;
        
        let store = self.store.read().await;
        
        match store.read_note(date) {
            Ok(note) => {
                let output = format!(
                    "## {}\n\n{}\n\n### Observations:\n",
                    note.date, 
                    note.content,
                    // TODO: format observations
                );
                
                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text { text: output }],
                    is_error: Some(false),
                })
            },
            Err(crate::error::MemoryError::NotFound(_)) => {
                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text {
                        text: format!("No note found for date: {}", date),
                    }],
                    is_error: Some(true),
                })
            },
            Err(e) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("Error reading note: {}", e),
                }],
                is_error: Some(true),
            }),
        }
    }
    
    pub async fn search_notes(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(arguments)
            .map_err(|e| CallToolError::ParseError(e.to_string()))?;
        
        let query = args.get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CallToolError::ParseError("missing required field: query".to_string()))?;
        
        let limit: usize = args.get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(10);
        
        let include_archived: bool = args.get("include_archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        
        // Embed the query
        let query_embedding = self.embedder.embed(query);
        
        let store = self.store.read().await;
        
        match store.search_notes(&query_embedding, limit, include_archived) {
            Ok(notes) => {
                let mut output = String::new();
                for note in &notes {
                    output.push_str(&format!(
                        "## {} - {}\n{}\n\n",
                        note.date,
                        note.metadata.title.as_deref().unwrap_or("Untitled"),
                        note.content.chars().take(200).collect::<String>()
                    ));
                }
                
                if notes.is_empty() {
                    output = "No notes found matching query".to_string();
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
    
    pub async fn recent_notes(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(arguments)
            .map_err(|e| CallToolError::ParseError(e.to_string()))?;
        
        let limit: usize = args.get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(10);
        
        let days: usize = args.get("days")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(7);
        
        let include_archived: bool = args.get("include_archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        
        let store = self.store.read().await;
        
        match store.get_recent_notes(limit, days, include_archived) {
            Ok(notes) => {
                let mut output = String::new();
                for note in &notes {
                    output.push_str(&format!(
                        "- {}\n",
                        note.metadata.title.as_deref().unwrap_or(&note.date)
                    ));
                }
                
                if output.is_empty() {
                    output = "No recent notes found".to_string();
                } else {
                    output = format!("Recent notes (last {} days):\n{}", days, output);
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
    
    pub async fn build_context(&self, arguments: serde_json::Value) -> Result<CallToolResult> {
        let args = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(arguments)
            .map_err(|e| CallToolError::ParseError(e.to_string()))?;
        
        let date = args.get("date")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CallToolError::ParseError("missing required field: date".to_string()))?;
        
        let category_filter: Option<String> = args.get("category_filter")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        
        let store = self.store.read().await;
        
        match store.read_note(date) {
            Ok(note) => {
                let mut output = format!("## {}\n\n{}

### Observations by Category:\n", 
                    note.date,
                    note.content);
                
                // Group observations by category
                let mut by_category: std::collections::HashMap<String, Vec<_>> = 
                    std::collections::HashMap::new();
                
                for obs in &note.observations {
                    let cat = obs.category.as_deref().unwrap_or("uncategorized");
                    by_category.entry(cat.to_string()).or_default().push(obs);
                }
                
                if let Some(filter) = category_filter {
                    if let Some(observations) = by_category.get(&filter) {
                        for obs in observations {
                            output.push_str(&format!(
                                "- [{}] {}\n",
                                obs.category.as_deref().unwrap_or(""),
                                obs.content
                            ));
                        }
                    } else {
                        output.push_str(&format!("No observations found with category: {}\n", filter));
                    }
                } else {
                    for (cat, observations) in by_category {
                        output.push_str(&format!("\n#### {}\n", cat));
                        for obs in observations {
                            output.push_str(&format!(
                                "- {}\n",
                                obs.content
                            ));
                        }
                    }
                }
                
                Ok(CallToolResult {
                    content: vec![mcp::protocol::ContentPart::Text { text: output }],
                    is_error: Some(false),
                })
            },
            Err(_) => Ok(CallToolResult {
                content: vec![mcp::protocol::ContentPart::Text {
                    text: format!("No note found for date: {}", date),
                }],
                is_error: Some(true),
            }),
        }
    }
}
```

**Acceptance Criteria for Tool Implementation:**
- All 5 tools can be called without panicking
- `write_note` returns error when file exists
- `read_note` returns appropriate error when note doesn't exist
- `search_notes` returns at least an empty list when no matches
- `recent_notes` respects `include_archived` flag
- `build_context` correctly filters by category

---

## Step 8: Main Entry Point

**File:** `src/main.rs`

```rust
mod error;
mod memory;
mod mcp;

use clap::Parser;
use std::path::PathBuf;
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "total-recall")]
#[command(about = "Agentic memory MCP server with SQLite + vector search")]
struct Args {
    /// Path to the memory directory (stores markdown files)
    #[arg(short, long, default_value = "~/.total-recall")]
    memory_dir: PathBuf,

    /// Path to SQLite database
    #[arg(short, long, default_value = "~/.total-recall/memory.db")]
    db_path: PathBuf,

    /// Logging level (debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,
}

fn expand_tilde(path: &PathBuf) -> PathBuf {
    if let Some(stripped) = path.to_str().and_then(|s| s.strip_prefix("~/")) {
        std::env::var("HOME")
            .map(|h| PathBuf::from(h).join(stripped))
            .unwrap_or_else(|_| path.clone())
    } else {
        path.clone()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let memory_dir = expand_tilde(&args.memory_dir);
    let db_path = expand_tilde(&args.db_path);

    // Create memory directory if it doesn't exist
    std::fs::create_dir_all(&memory_dir)?;

    // Setup logging
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::try_from_str(&args.log_level).unwrap_or_else(|_| {"info".into()})))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Initialize database
    tracing::info!("Initializing memory store at: {:?}", db_path);
    let store = memory::store::MemoryStore::new(&db_path)?;
    
    tracing::info!("Memory directory: {:?}", memory_dir);

    // Create MCP server
    let server = mcp::server::MemoryMcpServer::new(store, memory_dir);

    // Run MCP server
    tracing::info!("Starting MCP server...");
    
    // Use stdio transport (most compatible with Claude Desktop)
    let (_, handle) = mcp::server::serve(server, mcp::transport::stdio::StdIoTransport::new()?)
        .await?;
    
    handle.await;

    Ok(())
}
```

**Create test file:** `tests/integration_tests.rs`

```rust
use total_recall::memory::store::MemoryStore;
use tempfile::TempDir;

#[tokio::test]
async fn test_full_workflow() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test.db");
    
    let store = MemoryStore::new(&db_path).unwrap();
    
    // 1. Create note
    let content = r#"## 14:30
## Discussion
- [decision] Test decision content
- [action] Task to do
- [note] General note"#;

    let note = store.create_note("03/06/2026", content).unwrap();
    assert_eq!(note.date, "03/06/2026");
    
    // 2. Read note back
    let read_note = store.read_note("03/06/2026").unwrap();
    assert!(read_note.content.contains("Test decision content"));
    
    // 3. Try to create duplicate (should fail)
    let result = store.create_note("03/06/2026", "# Different");
    assert!(result.is_err());
    
    // 4. Archive and restore
    store.archive_note("03/06/2026").unwrap();
    let archived = store.read_note("03/06/2026").unwrap();
    assert!(archived.archived);
    
    store.restore_note("03/06/2026").unwrap();
    let restored = store.read_note("03/06/2026").unwrap();
    assert!(!restored.archived);
    
    // 5. Get recent notes
    let recent = store.get_recent_notes(10, 10, false).unwrap();
    assert!(recent.is_empty()); // All archived
    
    let recent_with_archive = store.get_recent_notes(10, 10, true).unwrap();
    assert_eq!(recent_with_archive.len(), 1);
}

#[tokio::test]
fn test_observation_parsing() {
    use total_recall::memory::file_parser::FileParser;
    
    let content = r#"## Team Sync
## 14:30
- [decision] Adopt Rust
- [action] John to review PR #urgent

## 15:00
- [idea] New feature idea"#;

    let observations = FileParser::parse_observations(content).unwrap();
    
    assert_eq!(observations.len(), 3);
    assert_eq!(observations[0].category, Some("decision".to_string()));
    assert_eq!(observations[0].content, "Adopt Rust");
    assert_eq!(observations[1].category, Some("action".to_string()));
    assert_eq!(observations[2].category, Some("idea".to_string()));
}
```

**Run all integration tests:**
```bash
cargo test --test integration_tests
# All tests must pass
```

---

## Step 9: Final Integration and Testing

**Goal:** Run complete test suite

**Commands:**

```bash
cd /Users/jgavinray/dev/memory/total-recall

# Run all tests
cargo test --all

# Check for warnings
cargo clippy

# Format code
cargo fmt

# Create binary
cargo build --release
```

**Acceptance Criteria for Complete System:**

1. **All tests pass:**
   ```bash
   cargo test --all
   # 20+ tests, all green
   ```

2. **Binary creates and runs:**
   ```bash
   ./target/release/total-recall --help
   ```

3. **MCP server responds:**
   ```bash
   # Run in separate terminal
   ./target/release/total-recall --memory-dir ~/test-memory
   
   # In another terminal
   curl -X POST http://localhost:3000/tools/list --header 'Content-Type: application/json' -d '{}'
   # Should return 5 tools
   ```

**Verification Checklist:**

- [ ] All unit tests pass (`cargo test --lib`)
- [ ] All integration tests pass (`cargo test --test '*'`)  
- [ ] No clippy warnings (`cargo clippy --all-targets`)
- [ ] Code is formatted (`cargo fmt -- --check`)
- [ ] Binary builds (`cargo build --release`)
- [ ] Help works (`./target/release/total-recall --help`)
- [ ] Memory directory can be created
- [ ] Database file is created

---

## Step 10: Configuration Files

**File to create:** `.claude_desktop_config.json` (for local testing)

```json
{
  "mcpServers": {
    "total-recall": {
      "command": "total-recall",
      "args": [
        "--memory-dir",
        "~/.total-recall",
        "--log-level",
        "debug"
      ],
      "env": {}
    }
  }
}
```

---

## Next Steps After Implementation

Once this implementation is complete:

1. Download actual ONNX model:
   ```bash
   # Get sentence-transformers/all-MminiLM-L6-v2
   # Replace placeholder embedder with real ONNX inference
   ```

2. Add more observation categories
3. Implement auto-tagging of observations
4. Add file watching for real-time sync
5. Write comprehensive README with examples
