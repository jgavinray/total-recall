use super::{MemoryStore, schema};
use crate::error::{MemoryError, Result};
use crate::memory::file_parser::FileParser;
use crate::memory::models::Observation;
use chrono::Utc;
use rusqlite::params;

impl MemoryStore {
    pub fn parse_and_insert_observations(
        &self,
        date: &str,
        content: &str,
    ) -> Result<Vec<Observation>> {
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
            let embedding_json = schema::embedding_json(&embedding);

            conn.execute(
                "INSERT INTO vec_observations(rowid, embedding) VALUES (?1, vec_f32(?2))",
                params![obs_rowid, embedding_json],
            )?;

            inserted.push(obs);
        }

        Ok(inserted)
    }

    /// Insert a raw text chunk as a synthetic observation so it's vector-searchable.
    /// Used when the content doesn't contain structured `- [category]` observations.
    pub(super) fn insert_raw_observation(&self, date: &str, content: &str) -> Result<()> {
        // Strip leading/trailing whitespace and skip empty
        let text = content.trim();
        if text.is_empty() {
            return Ok(());
        }

        let obs_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().timestamp().to_string();

        let conn = self.connection.lock().unwrap();
        conn.execute(
            "INSERT INTO observations (id, note_id, timestamp, section, category, content, context, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                obs_id,
                date,
                now,
                Option::<String>::None,
                "memory",
                text,
                text,
                "[]"
            ],
        )?;

        let obs_rowid = conn.last_insert_rowid();
        drop(conn); // Release lock before embedding (which doesn't need it)

        // Compute and store embedding
        let embedding = self.embedder.embed(text);
        let embedding_json = schema::embedding_json(&embedding);

        self.connection.lock().unwrap().execute(
            "INSERT INTO vec_observations(rowid, embedding) VALUES (?1, vec_f32(?2))",
            params![obs_rowid, embedding_json],
        )?;

        Ok(())
    }

    pub(super) fn get_observations_for_note(&self, date: &str) -> Result<Vec<Observation>> {
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
            .map_err(|e| MemoryError::ParseError(format!("Failed to parse observations: {}", e)))
    }
}
