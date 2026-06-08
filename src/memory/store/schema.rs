use crate::error::{MemoryError, Result};
use crate::memory::embedder::Embedder;
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::Once;

/// Register sqlite-vec extension for all new SQLite connections (once per process).
static SQLITE_VEC_LOADED: Once = Once::new();

pub(super) fn ensure_sqlite_vec_loaded() {
    SQLITE_VEC_LOADED.call_once(|| {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        tracing::info!("sqlite-vec extension registered via auto_extension");
    });
}

pub(super) fn ensure_vector_table(conn: &Connection, dimension: usize) -> Result<bool> {
    if dimension == 0 {
        return Err(MemoryError::Embedding(
            "embedding dimension must be greater than zero".to_string(),
        ));
    }

    let expected = format!("float[{dimension}]");
    let existing_sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='vec_observations'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    let mut recreated = false;
    if let Some(sql) = existing_sql {
        if !sql.replace(' ', "").contains(&expected) {
            tracing::warn!(
                expected_dimension = dimension,
                existing_sql = %sql,
                "Recreating sqlite-vec table because embedding dimension changed"
            );
            conn.execute_batch("DROP TABLE IF EXISTS vec_observations;")?;
            recreated = true;
        }
    } else {
        recreated = true;
    }

    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_observations USING vec0(embedding float[{dimension}]);"
    ))?;

    Ok(recreated)
}

pub(super) fn reindex_observations(conn: &Connection, embedder: &Embedder) -> Result<()> {
    let rows = {
        let mut stmt = conn.prepare("SELECT rowid, content FROM observations ORDER BY rowid")?;
        let mapped = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        mapped.collect::<std::result::Result<Vec<_>, _>>()?
    };

    if rows.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = rows.len(),
        "Reindexing observations for embedding model"
    );
    for (rowid, content) in rows {
        let embedding = embedder.embed(&content);
        let embedding_json = embedding_json(&embedding);
        conn.execute(
            "INSERT OR REPLACE INTO vec_observations(rowid, embedding) VALUES (?1, vec_f32(?2))",
            params![rowid, embedding_json],
        )?;
    }

    Ok(())
}

pub(super) fn embedding_json(embedding: &[f32]) -> String {
    format!(
        "[{}]",
        embedding
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}
