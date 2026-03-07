use crate::error::{MemoryError, Result};
use crate::memory::models::Observation;

pub struct FileParser;

impl FileParser {
    pub fn parse_observations(content: &str) -> Result<Vec<Observation>> {
        let mut observations = Vec::new();
        let mut current_section = None;
        let mut current_timestamp = String::new();

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with("---") {
                continue;
            }

            if trimmed.starts_with("## ") {
                let section_title = &trimmed[3..];
                if Self::is_timestamp(section_title) {
                    current_timestamp = section_title.to_string();
                } else {
                    current_section = Some(section_title.to_string());
                }
                continue;
            }

            if trimmed.starts_with("- [") {
                if let Some(obs) =
                    Self::parse_observation_line(trimmed, &current_timestamp, &current_section)
                {
                    observations.push(obs);
                }
            }
        }

        Ok(observations)
    }

    fn is_timestamp(s: &str) -> bool {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return false;
        }
        parts[0].chars().all(|c| c.is_ascii_digit()) && parts[1].chars().all(|c| c.is_ascii_digit())
    }

    fn parse_observation_line(
        line: &str,
        timestamp: &str,
        section: &Option<String>,
    ) -> Option<Observation> {
        let content = line.trim_start().trim_start_matches("- ");

        let end_bracket = content.find(']')?;
        let category = content[1..end_bracket].to_string();
        let mut content_text = content[end_bracket + 2..].to_string();

        let mut tags = Vec::new();
        let mut cleaned = content_text.clone();

        for token in content_text.split('#') {
            let token = token.trim();
            let parts: Vec<&str> = token
                .split(|c: char| !c.is_alphanumeric() && c != '-')
                .collect();
            if let Some(first) = parts.first() {
                if !first.is_empty() {
                    tags.push(first.to_string());
                }
            }
            cleaned = cleaned.trim_start_matches(token).to_string();
        }
        cleaned = cleaned.trim().replace("#", "").trim().to_string();
        content_text = cleaned;

        let mut full_context = line.to_string();
        full_context.truncate(full_context.trim_end().len());

        Some(Observation {
            id: uuid::Uuid::new_v4().to_string(),
            note_id: String::new(),
            timestamp: timestamp.to_string(),
            section: section.clone(),
            category: Some(category),
            content: content_text,
            full_context,
            tags,
        })
    }
}
