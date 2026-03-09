use crate::error::{MemoryError, Result};
use crate::memory::embedder::Embedder;
use crate::memory::models::{Note, NoteMetadata, Observation};
use chrono::Utc;
use std::sync::Once;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

/// Register sqlite-vec extension for all new SQLite connections (once per process).
static SQLITE_VEC_LOADED: Once = Once::new();

fn ensure_sqlite_vec_loaded() {
    SQLITE_VEC_LOADED.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        tracing::info!("sqlite-vec extension registered via auto_extension");
    });
}

pub struct MemoryStore {
    connection: Arc<Mutex<Connection>>,
    embedder: Embedder,
}

#[derive(Clone)]
pub struct EmbeddingRow {
    pub id: String,
    pub note_id: String,
    pub timestamp: String,
    pub section: Option<String>,
    pub category: Option<String>,
    pub content: String,
    pub context: String,
    pub tags: Vec<String>,
    pub embedding: Vec<f32>,
}

impl MemoryStore {
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Register sqlite-vec before opening any connection
        ensure_sqlite_vec_loaded();

        let conn = Connection::open(db_path)?;

        // Verify sqlite-vec loaded correctly
        let vec_version: String = conn
            .query_row("SELECT vec_version()", [], |r| r.get(0))
            .map_err(|e| MemoryError::ParseError(format!("sqlite-vec not loaded: {}", e)))?;
        tracing::info!("sqlite-vec version: {}", vec_version);

        conn.execute("PRAGMA journal_mode=WAL", [])?;

        // Migrate: drop old vss-based virtual table if present (incompatible schema)
        let is_vss: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='observations' AND sql LIKE '%vss%'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0;

        if is_vss {
            tracing::warn!(
                "Dropping old sqlite-vss 'observations' table and migrating to sqlite-vec"
            );
            conn.execute_batch(
                "DROP TABLE IF EXISTS observations;
                 DROP TABLE IF EXISTS vec_observations;",
            )?;
        }

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                date TEXT NOT NULL UNIQUE,
                title TEXT,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                archived INTEGER DEFAULT 0
            );

            -- Regular observations table (metadata only)
            CREATE TABLE IF NOT EXISTS observations (
                id TEXT NOT NULL UNIQUE,
                note_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                section TEXT,
                category TEXT,
                content TEXT NOT NULL,
                context TEXT NOT NULL,
                tags TEXT
            );

            -- Vector index table using sqlite-vec (vec0 virtual table)
            -- rowid matches observations.rowid for joining
            CREATE VIRTUAL TABLE IF NOT EXISTS vec_observations USING vec0(
                embedding float[384]
            );

            CREATE INDEX IF NOT EXISTS idx_observations_note_id ON observations(note_id);
            CREATE INDEX IF NOT EXISTS idx_observations_category ON observations(category);
            CREATE INDEX IF NOT EXISTS idx_notes_date ON notes(date DESC);
            CREATE INDEX IF NOT EXISTS idx_notes_archived ON notes(archived);
            ",
        )?;

        tracing::info!("MemoryStore initialized with sqlite-vec vector search");

        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            embedder: Embedder::new()?,
        })
    }

    pub fn parse_and_insert_observations(
        &self,
        date: &str,
        content: &str,
    ) -> Result<Vec<Observation>> {
        use crate::memory::file_parser::FileParser;
        let observations = FileParser::parse_observations(content)?;

        let mut inserted = Vec::new();
        for mut obs in observations {
            obs.note_id = date.to_string();
            let obs_id = uuid::Uuid::new_v4().to_string();

            let conn = self.connection.lock().unwrap();

            // Insert observation metadata
            conn.execute(
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
                    serde_json::to_string(&obs.tags).unwrap_or("[]".to_string())
                ],
            )?;

            let obs_rowid = conn.last_insert_rowid();

            // Compute and store embedding in vec_observations
            let embedding = self.embedder.embed(&obs.content);
            let embedding_json = format!(
                "[{}]",
                embedding
                    .iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            );

            conn.execute(
                "INSERT INTO vec_observations(rowid, embedding) VALUES (?1, vec_f32(?2))",
                params![obs_rowid, embedding_json],
            )?;

            inserted.push(obs);
        }

        Ok(inserted)
    }

    pub fn create_note(&self, date: &str, content: &str) -> Result<Note> {
        let exists: i64 = self.connection.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM notes WHERE date = ?",
            [date],
            |row| row.get(0),
        )?;

        if exists > 0 {
            return Err(MemoryError::FileExistsError(
                date.to_string().replace('/', "-").into(),
            ));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();

        let metadata = NoteMetadata::parse_frontmatter(content).unwrap_or_default();
        let title = metadata.title.clone().unwrap_or_else(|| date.to_string());

        self.connection.lock().unwrap().execute(
            "INSERT INTO notes (id, date, title, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, date, title, content, now, now],
        )?;

        self.parse_and_insert_observations(date, content)?;

        self.read_note(date)
    }

    pub fn read_note(&self, date: &str) -> Result<Note> {
        let row = self.connection.lock().unwrap().query_row(
            "SELECT id, date, title, content, created_at, updated_at, archived FROM notes WHERE date = ?",
            [date],
            |row| Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            )),
        )?;

        let (id, date_str, title, content, created_at, updated_at, archived) = row;

        let observations = self.get_observations_for_note(&date_str)?;

        let metadata = NoteMetadata {
            title: title.clone(),
            date: Some(date_str.clone()),
            r#type: None,
            tags: None,
            archived: Some(archived > 0),
        };

        Ok(Note {
            id,
            date: date_str,
            metadata,
            content,
            observations,
            created_at,
            updated_at,
            archived: archived > 0,
        })
    }

    fn get_observations_for_note(&self, date: &str) -> Result<Vec<Observation>> {
        let conn = self.connection.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, note_id, timestamp, section, category, content, context, tags FROM observations WHERE note_id = ?",
        )?;

        let obs_rows = stmt.query_map([date], |row| {
            let id: String = row.get(0)?;
            let note_id: String = row.get(1)?;
            let timestamp: String = row.get(2)?;
            let section: Option<String> = row.get(3)?;
            let category: Option<String> = row.get(4)?;
            let content: String = row.get(5)?;
            let full_context: String = row.get(6)?;
            let tags_json: String = row.get(7)?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

            Ok(Observation {
                id,
                note_id,
                timestamp,
                section,
                category,
                content,
                full_context,
                tags,
            })
        })?;

        obs_rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| {
                crate::error::MemoryError::ParseError(format!(
                    "Failed to parse observations: {}",
                    e
                ))
            })
    }

    pub fn archive_note(&self, date: &str) -> Result<()> {
        self.connection
            .lock()
            .unwrap()
            .execute("UPDATE notes SET archived = 1 WHERE date = ?", [date])?;
        Ok(())
    }

    pub fn restore_note(&self, date: &str) -> Result<()> {
        self.connection
            .lock()
            .unwrap()
            .execute("UPDATE notes SET archived = 0 WHERE date = ?", [date])?;
        Ok(())
    }

    pub fn get_recent_notes(
        &self,
        limit: usize,
        days: usize,
        include_archived: bool,
    ) -> Result<Vec<Note>> {
        let days_ago = Utc::now().timestamp() - (days as i64 * 86400);

        let query = if include_archived {
            "SELECT id, date, title, content, created_at, updated_at, archived FROM notes WHERE updated_at >= ? ORDER BY updated_at DESC LIMIT ?"
        } else {
            "SELECT id, date, title, content, created_at, updated_at, archived FROM notes WHERE updated_at >= ? AND archived = 0 ORDER BY updated_at DESC LIMIT ?"
        };

        let conn = self.connection.lock().unwrap();
        let mut stmt = conn.prepare(query)?;

        let note_rows = stmt.query_map([days_ago, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;

        let mut notes = Vec::new();
        for row in note_rows {
            let (id, date_str, title, content, created_at, updated_at, archived) = row?;
            notes.push(Note {
                id,
                date: date_str.clone(),
                metadata: NoteMetadata {
                    title: title.clone(),
                    date: Some(date_str),
                    r#type: None,
                    tags: None,
                    archived: Some(archived > 0),
                },
                content,
                observations: Vec::new(),
                created_at,
                updated_at,
                archived: archived > 0,
            });
        }

        Ok(notes)
    }

    pub fn search_notes(
        &self,
        query_embedding: &[f32],
        limit: usize,
        include_archived: bool,
    ) -> Result<Vec<Note>> {
        let embedding_json = format!(
            "[{}]",
            query_embedding
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        // sqlite-vec KNN search: find closest observations, join back to notes
        let query = if include_archived {
            "
            SELECT DISTINCT n.id, n.date, n.title, n.content, n.updated_at, n.archived,
                   MIN(v.distance) as min_distance
            FROM vec_observations v
            JOIN observations o ON o.rowid = v.rowid
            JOIN notes n ON n.date = o.note_id
            WHERE v.embedding MATCH vec_f32(?1)
            ORDER BY v.distance
            LIMIT ?2
            "
        } else {
            "
            SELECT DISTINCT n.id, n.date, n.title, n.content, n.updated_at, n.archived,
                   MIN(v.distance) as min_distance
            FROM vec_observations v
            JOIN observations o ON o.rowid = v.rowid
            JOIN notes n ON n.date = o.note_id
            WHERE v.embedding MATCH vec_f32(?1) AND n.archived = 0
            ORDER BY v.distance
            LIMIT ?2
            "
        };

        let conn = self.connection.lock().unwrap();
        let mut stmt = conn.prepare(query)?;

        let note_rows = stmt.query_map(params![embedding_json, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;

        let mut notes = Vec::new();
        for row in note_rows {
            let (id, date_str, title, content, updated_at, archived) = row?;
            notes.push(Note {
                id,
                date: date_str.clone(),
                metadata: NoteMetadata {
                    title: title.clone(),
                    date: Some(date_str),
                    r#type: None,
                    tags: None,
                    archived: Some(archived > 0),
                },
                content,
                observations: Vec::new(),
                created_at: updated_at,
                updated_at,
                archived: archived > 0,
            });
        }

        Ok(notes)
    }
}
