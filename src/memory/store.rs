use crate::error::{MemoryError, Result};
use crate::memory::models::{Note, NoteMetadata, Observation};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Arc;

pub struct MemoryStore {
    connection: Arc<Connection>,
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

        let conn = Connection::open(db_path)?;

        // Load VSS extension
        #[cfg(unix)]
        {
            conn.load_extension("sqlite3_vss")?;
        }

        conn.execute("PRAGMA journal_mode=WAL", [])?;

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

            CREATE VIRTUAL TABLE IF NOT EXISTS observations USING vss(
                id TEXT PRIMARY KEY,
                note_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                section TEXT,
                category TEXT,
                content TEXT NOT NULL,
                context TEXT NOT NULL,
                tags TEXT,
                embedding FLOAT[384]
            );

            CREATE INDEX IF NOT EXISTS idx_observations_note_id ON observations(note_id);
            CREATE INDEX IF NOT EXISTS idx_observations_category ON observations(category);
            CREATE INDEX IF NOT EXISTS idx_notes_date ON notes(date DESC);
            CREATE INDEX IF NOT EXISTS idx_notes_archived ON notes(archived);
            ",
        )?;

        Ok(Self {
            connection: Arc::new(conn),
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
                    serde_json::to_string(&obs.tags).unwrap_or("[]".to_string())
                ],
            )?;

            inserted.push(obs);
        }

        Ok(inserted)
    }

    pub fn create_note(&self, date: &str, content: &str) -> Result<Note> {
        let exists: i64 = self.connection.query_row(
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

        self.connection.execute(
            "INSERT INTO notes (id, date, title, content, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, date, title, content, now, now],
        )?;

        self.parse_and_insert_observations(date, content)?;

        self.read_note(date)
    }

    pub fn read_note(&self, date: &str) -> Result<Note> {
        let row = self.connection.query_row(
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

        let (id, date, title, content, created_at, updated_at, archived) = row;

        let observations = self.get_observations_for_note(date)?;

        let metadata = NoteMetadata {
            title: Some(title.clone()),
            date: Some(date.clone()),
            r#type: None,
            tags: None,
            archived: Some(archived > 0),
        };

        Ok(Note {
            id,
            date,
            metadata,
            content,
            observations,
            created_at,
            updated_at,
            archived: archived > 0,
        })
    }

    fn get_observations_for_note(&self, date: &str) -> Result<Vec<Observation>> {
        let mut stmt = self.connection.prepare(
            "SELECT id, note_id, timestamp, section, category, content, context, tags FROM observations WHERE note_id = ?",
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

        obs_rows.collect()
    }

    pub fn archive_note(&self, date: &str) -> Result<()> {
        self.connection
            .execute("UPDATE notes SET archived = 1 WHERE date = ?", [date])?;
        Ok(())
    }

    pub fn restore_note(&self, date: &str) -> Result<()> {
        self.connection
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

        let mut stmt = self.connection.prepare(query)?;

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
            let (id, date, title, content, created_at, updated_at, archived) = row?;
            notes.push(Note {
                id,
                date,
                metadata: NoteMetadata {
                    title: title.clone(),
                    date: Some(date.clone()),
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
        let query = if include_archived {
            "
            SELECT n.id, n.date, n.title, n.content, n.updated_at, n.archived, 
                   AVG(o.distance) as avg_distance
            FROM notes n
            JOIN observations o ON n.date = o.note_id
            WHERE o.embedding MATCH ?1 AND k = ?2
            GROUP BY n.date
            ORDER BY avg_distance
            LIMIT ?3
            "
        } else {
            "
            SELECT n.id, n.date, n.title, n.content, n.updated_at, n.archived,
                   AVG(o.distance) as avg_distance
            FROM notes n
            JOIN observations o ON n.date = o.note_id
            WHERE o.embedding MATCH ?1 AND k = ?2 AND n.archived = 0
            GROUP BY n.date
            ORDER BY avg_distance
            LIMIT ?3
            "
        };

        let mut stmt = self.connection.prepare(query)?;

        let note_rows = stmt.query_map(
            [
                serde_json::to_string(query_embedding)
                    .map_err(|e| MemoryError::Embedding(e.to_string()))?,
                limit as i64,
                limit as i64,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;

        let mut notes = Vec::new();
        for row in note_rows {
            let (id, date, title, content, updated_at, archived) = row?;
            notes.push(Note {
                id,
                date,
                metadata: NoteMetadata {
                    title: title.clone(),
                    date: Some(date.clone()),
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
