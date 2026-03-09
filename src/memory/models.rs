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

#[cfg(test)]
mod tests {
    use super::*;

    // --- NoteMetadata::parse_frontmatter tests ---

    #[test]
    fn test_parse_frontmatter_full() {
        let content = "---\ntitle: My Note\ndate: 2026-03-09\ntype: daily\ntags:\n  - rust\n  - testing\narchived: false\n---\nBody text";
        // Safe: known-good YAML; parse_frontmatter should never panic on well-formed input
        let meta = NoteMetadata::parse_frontmatter(content).unwrap();
        assert_eq!(meta.title, Some("My Note".to_string()));
        assert_eq!(meta.date, Some("2026-03-09".to_string()));
        assert_eq!(meta.r#type, Some("daily".to_string()));
        assert_eq!(meta.tags, Some(vec!["rust".to_string(), "testing".to_string()]));
        assert_eq!(meta.archived, Some(false));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "No frontmatter here, just plain text.";
        // Safe: missing frontmatter should return a default, not an error
        let meta = NoteMetadata::parse_frontmatter(content).unwrap();
        assert!(meta.title.is_none());
        assert!(meta.date.is_none());
    }

    #[test]
    fn test_parse_frontmatter_partial() {
        let content = "---\ntitle: Partial\n---\nSome content";
        // Safe: partial frontmatter with only title is valid YAML
        let meta = NoteMetadata::parse_frontmatter(content).unwrap();
        assert_eq!(meta.title, Some("Partial".to_string()));
        assert!(meta.tags.is_none());
        assert!(meta.archived.is_none());
    }

    #[test]
    fn test_parse_frontmatter_empty_frontmatter() {
        let content = "---\n---\nBody";
        // Safe: empty YAML block should deserialize to all-None defaults
        let meta = NoteMetadata::parse_frontmatter(content).unwrap();
        assert!(meta.title.is_none());
    }

    // --- NoteMetadata default tests ---

    #[test]
    fn test_note_metadata_default() {
        let meta = NoteMetadata::default();
        assert!(meta.title.is_none());
        assert!(meta.date.is_none());
        assert!(meta.r#type.is_none());
        assert!(meta.tags.is_none());
        assert!(meta.archived.is_none());
    }

    // --- Observation struct tests ---

    #[test]
    fn test_observation_clone_and_debug() {
        let obs = Observation {
            id: "abc".to_string(),
            note_id: "2026-03-09".to_string(),
            timestamp: "10:30".to_string(),
            section: Some("Work".to_string()),
            category: Some("task".to_string()),
            content: "Finished the tests".to_string(),
            full_context: "- [task] Finished the tests".to_string(),
            tags: vec!["rust".to_string()],
        };
        let cloned = obs.clone();
        assert_eq!(obs.id, cloned.id);
        assert_eq!(obs.tags, cloned.tags);
        // Ensure Debug is implemented (should compile and not panic)
        let _ = format!("{:?}", obs);
    }

    // --- Note struct tests ---

    #[test]
    fn test_note_archived_field() {
        let note = Note {
            id: "id1".to_string(),
            date: "2026-03-09".to_string(),
            metadata: NoteMetadata::default(),
            content: "test".to_string(),
            observations: vec![],
            created_at: 1000,
            updated_at: 2000,
            archived: true,
        };
        assert!(note.archived);
        assert_eq!(note.created_at, 1000);
    }
}
