use super::{MemoryStore, schema};
use crate::error::Result;
use crate::memory::models::{Note, NoteMetadata};
use chrono::Utc;
use rusqlite::params;

impl MemoryStore {
    pub fn embed_query(&self, query: &str) -> Vec<f32> {
        self.embedder.embed(query)
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
        let embedding_json = schema::embedding_json(query_embedding);

        // sqlite-vec KNN search requires LIMIT inside the KNN subquery.
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
