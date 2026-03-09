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

        // PRAGMA journal_mode=WAL returns a row with the mode name; use execute_batch to ignore it.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

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

        // sqlite-vec KNN search: vec0 virtual tables require the LIMIT to be pushed down
        // directly onto the KNN subquery (CTE) — a bare JOIN with outer LIMIT is not enough.
        // We use a WITH clause to first fetch the nearest rowids, then join to notes.
        let query = if include_archived {
            "
            WITH knn AS (
                SELECT rowid, distance
                FROM vec_observations
                WHERE embedding MATCH vec_f32(?1)
                LIMIT ?2
            )
            SELECT DISTINCT n.id, n.date, n.title, n.content, n.updated_at, n.archived
            FROM knn k
            JOIN observations o ON o.rowid = k.rowid
            JOIN notes n ON n.date = o.note_id
            ORDER BY k.distance
            "
        } else {
            "
            WITH knn AS (
                SELECT rowid, distance
                FROM vec_observations
                WHERE embedding MATCH vec_f32(?1)
                LIMIT ?2
            )
            SELECT DISTINCT n.id, n.date, n.title, n.content, n.updated_at, n.archived
            FROM knn k
            JOIN observations o ON o.rowid = k.rowid
            JOIN notes n ON n.date = o.note_id
            WHERE n.archived = 0
            ORDER BY k.distance
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a fresh MemoryStore backed by a temp directory.
    /// Safe to unwrap: TempDir and MemoryStore::new are expected to succeed in a clean test env.
    fn make_store() -> (TempDir, MemoryStore) {
        // Safe: tempdir() only fails on OS errors (out of space, permissions),
        // not expected in a normal test environment.
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        // Safe: db_path is a fresh temp path with no contention.
        let store = MemoryStore::new(&db_path).unwrap();
        (dir, store)
    }

    /// Sample note content with one observation.
    fn sample_content() -> &'static str {
        "---\ntitle: Test Note\n---\n## Work\n## 10:00\n- [task] Buy milk #shopping\n"
    }

    // --- create_note tests ---

    #[test]
    fn test_create_note_returns_note() {
        let (_dir, store) = make_store();
        // Safe: make_store always succeeds in test env; fresh store has no date conflict.
        let note = store.create_note("2026-03-09", sample_content()).unwrap();
        assert_eq!(note.date, "2026-03-09");
        assert!(!note.id.is_empty());
        assert!(!note.archived);
    }

    #[test]
    fn test_create_note_title_from_frontmatter() {
        let (_dir, store) = make_store();
        let note = store.create_note("2026-03-10", sample_content()).unwrap();
        assert_eq!(note.metadata.title, Some("Test Note".to_string()));
    }

    #[test]
    fn test_create_note_duplicate_returns_error() {
        let (_dir, store) = make_store();
        // Safe: first insert must succeed; second on same date should fail with FileExistsError.
        store.create_note("2026-03-09", sample_content()).unwrap();
        let result = store.create_note("2026-03-09", sample_content());
        assert!(result.is_err(), "duplicate date should return an error");
    }

    // --- read_note tests ---

    #[test]
    fn test_read_note_after_create() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: just created above, must exist.
        let note = store.read_note("2026-03-09").unwrap();
        assert_eq!(note.date, "2026-03-09");
        assert_eq!(note.content, sample_content());
    }

    #[test]
    fn test_read_note_not_found_returns_error() {
        let (_dir, store) = make_store();
        let result = store.read_note("9999-99-99");
        assert!(result.is_err(), "reading non-existent note should fail");
    }

    #[test]
    fn test_read_note_includes_observations() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: note was just created above.
        let note = store.read_note("2026-03-09").unwrap();
        assert!(
            !note.observations.is_empty(),
            "note should have at least one observation"
        );
    }

    // --- archive / restore tests ---

    #[test]
    fn test_archive_note() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: note exists; archive should succeed.
        store.archive_note("2026-03-09").unwrap();
        let note = store.read_note("2026-03-09").unwrap();
        assert!(note.archived, "note should be archived after archive_note()");
    }

    #[test]
    fn test_restore_note() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: archive then restore; both operations on existing note.
        store.archive_note("2026-03-09").unwrap();
        store.restore_note("2026-03-09").unwrap();
        let note = store.read_note("2026-03-09").unwrap();
        assert!(!note.archived, "note should not be archived after restore_note()");
    }

    // --- get_recent_notes tests ---

    #[test]
    fn test_get_recent_notes_returns_created() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: limit=10, days=1 (note was just created with Utc::now() timestamp).
        let notes = store.get_recent_notes(10, 1, false).unwrap();
        assert!(!notes.is_empty(), "should return the recently created note");
        assert!(notes.iter().any(|n| n.date == "2026-03-09"));
    }

    #[test]
    fn test_get_recent_notes_excludes_archived() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        store.archive_note("2026-03-09").unwrap();
        // Safe: note is archived; include_archived=false should exclude it.
        let notes = store.get_recent_notes(10, 1, false).unwrap();
        assert!(
            !notes.iter().any(|n| n.date == "2026-03-09"),
            "archived note should not appear when include_archived=false"
        );
    }

    #[test]
    fn test_get_recent_notes_includes_archived_when_flag_set() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        store.archive_note("2026-03-09").unwrap();
        // Safe: include_archived=true, should include the archived note.
        let notes = store.get_recent_notes(10, 1, true).unwrap();
        assert!(
            notes.iter().any(|n| n.date == "2026-03-09"),
            "archived note should appear when include_archived=true"
        );
    }

    #[test]
    fn test_get_recent_notes_respects_limit() {
        let (_dir, store) = make_store();
        for i in 1..=5 {
            let date = format!("2026-03-{:02}", i);
            store.create_note(&date, sample_content()).unwrap();
        }
        // Safe: 5 notes inserted; limit=3 should return at most 3.
        let notes = store.get_recent_notes(3, 1, false).unwrap();
        assert!(notes.len() <= 3, "should respect the limit parameter");
    }

    // --- search_notes tests ---

    #[test]
    fn test_search_notes_returns_relevant() {
        let (_dir, store) = make_store();
        store.create_note("2026-03-09", sample_content()).unwrap();
        // Safe: Embedder::new() succeeds (model cached); embed is deterministic.
        let embedder = Embedder::new().unwrap();
        let query_vec = embedder.embed("shopping task milk");
        // Safe: limit=5, non-archived search.
        let notes = store.search_notes(&query_vec, 5, false).unwrap();
        assert!(
            !notes.is_empty(),
            "semantic search should return at least one result"
        );
        assert!(
            notes.iter().any(|n| n.date == "2026-03-09"),
            "the note with matching content should appear in search results"
        );
    }

    #[test]
    fn test_search_notes_empty_store() {
        let (_dir, store) = make_store();
        // Safe: Embedder::new() succeeds with cached model.
        let embedder = Embedder::new().unwrap();
        let query_vec = embedder.embed("anything");
        // Safe: no notes inserted; result should be empty, not an error.
        let notes = store.search_notes(&query_vec, 5, false).unwrap();
        assert!(notes.is_empty(), "search on empty store should return empty vec");
    }
}
