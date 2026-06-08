use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use total_recall::memory;

#[derive(Clone)]
pub(crate) struct MemoryApiState {
    pub(crate) store: Arc<RwLock<memory::store::MemoryStore>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecentNotesQuery {
    days: Option<usize>,
    limit: Option<usize>,
    include_archived: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchNotesRequest {
    query: String,
    limit: Option<usize>,
    include_archived: Option<bool>,
}

#[derive(Debug, Serialize)]
struct NotesResponse {
    notes: Vec<MemoryNoteResponse>,
}

#[derive(Debug, Serialize)]
struct MemoryNoteResponse {
    id: String,
    date: String,
    title: Option<String>,
    content: String,
    updated_at: i64,
    archived: bool,
}

pub(crate) async fn api_recent_notes(
    axum::extract::State(state): axum::extract::State<MemoryApiState>,
    axum::extract::Query(query): axum::extract::Query<RecentNotesQuery>,
) -> axum::response::Response {
    let limit = query.limit.unwrap_or(10).clamp(1, 100);
    let days = query.days.unwrap_or(7).clamp(1, 3650);
    let include_archived = query.include_archived.unwrap_or(false);
    let store = state.store.read().await;
    match store.get_recent_notes(limit, days, include_archived) {
        Ok(notes) => axum::Json(NotesResponse {
            notes: notes.into_iter().map(MemoryNoteResponse::from).collect(),
        })
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "error": "recent_notes_failed",
                "detail": e.to_string()
            })),
        )
            .into_response(),
    }
}

pub(crate) async fn api_search_notes(
    axum::extract::State(state): axum::extract::State<MemoryApiState>,
    axum::Json(request): axum::Json<SearchNotesRequest>,
) -> axum::response::Response {
    let limit = request.limit.unwrap_or(10).clamp(1, 100);
    let include_archived = request.include_archived.unwrap_or(false);
    let store = state.store.read().await;
    let embedding = store.embed_query(&request.query);
    match store.search_notes(&embedding, limit, include_archived) {
        Ok(notes) => axum::Json(NotesResponse {
            notes: notes.into_iter().map(MemoryNoteResponse::from).collect(),
        })
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({
                "error": "search_notes_failed",
                "detail": e.to_string()
            })),
        )
            .into_response(),
    }
}

impl From<memory::models::Note> for MemoryNoteResponse {
    fn from(note: memory::models::Note) -> Self {
        Self {
            id: note.id,
            date: note.date,
            title: note.metadata.title,
            content: note.content,
            updated_at: note.updated_at,
            archived: note.archived,
        }
    }
}
