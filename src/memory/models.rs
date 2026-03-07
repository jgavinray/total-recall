use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteMetadata {
    pub title: Option<String>,
    pub date: Option<String>,
    pub r#type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub note_id: String,
    pub timestamp: String,
    pub section: Option<String>,
    pub category: Option<String>,
    pub content: String,
    pub full_context: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub date: String,
    pub metadata: NoteMetadata,
    pub content: String,
    pub observations: Vec<Observation>,
    pub created_at: i64,
    pub updated_at: i64,
    pub archived: bool,
}

impl NoteMetadata {
    pub fn parse_frontmatter(text: &str) -> Result<Self, crate::error::MemoryError> {
        let start = match text.find("---") {
            Some(s) => s,
            None => return Ok(NoteMetadata::default()),
        };

        let rest = &text[start + 3..];
        let end = match rest.find("---") {
            Some(e) => e,
            None => return Ok(NoteMetadata::default()),
        };

        let frontmatter = &rest[..end];
        serde_yaml::from_str(frontmatter).map_err(|e| {
            crate::error::MemoryError::ParseError(format!("failed to parse YAML: {}", e))
        })
    }
}

impl Default for NoteMetadata {
    fn default() -> Self {
        Self {
            title: None,
            date: None,
            r#type: None,
            tags: None,
            archived: None,
        }
    }
}
