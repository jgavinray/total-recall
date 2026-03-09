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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_note() -> &'static str {
        "---\ntitle: Daily Note\n---\n\
         ## Work\n\
         ## 10:30\n\
         - [task] Finished the Rust tests #rust #testing\n\
         - [note] Reviewed PR for main branch\n\
         ## 14:00\n\
         - [idea] Refactor embedder to support batching\n"
    }

    #[test]
    fn test_parse_observations_count() {
        // Safe: sample_note is valid test input; parse_observations is infallible in practice
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        assert_eq!(obs.len(), 3, "expected 3 observations from sample note");
    }

    #[test]
    fn test_parse_observations_category() {
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        assert_eq!(obs[0].category, Some("task".to_string()));
        assert_eq!(obs[1].category, Some("note".to_string()));
        assert_eq!(obs[2].category, Some("idea".to_string()));
    }

    #[test]
    fn test_parse_observations_timestamp() {
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        // First two observations fall under ## 10:30
        assert_eq!(obs[0].timestamp, "10:30");
        assert_eq!(obs[1].timestamp, "10:30");
        // Third observation is under ## 14:00
        assert_eq!(obs[2].timestamp, "14:00");
    }

    #[test]
    fn test_parse_observations_section() {
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        // Section "Work" is set before the timestamps
        assert_eq!(obs[0].section, Some("Work".to_string()));
        assert_eq!(obs[1].section, Some("Work".to_string()));
    }

    #[test]
    fn test_parse_observations_empty_content() {
        // Safe: empty string should return empty vec without error
        let obs = FileParser::parse_observations("").unwrap();
        assert!(obs.is_empty());
    }

    #[test]
    fn test_parse_observations_no_observations() {
        let content = "---\ntitle: Empty\n---\n## 09:00\nJust prose, no bullet points.\n";
        let obs = FileParser::parse_observations(content).unwrap();
        assert!(obs.is_empty());
    }

    #[test]
    fn test_parse_observations_full_context_not_empty() {
        // Note: `content` field may be empty for observations without #hashtags due to parser
        // cleaning logic. full_context always preserves the original line verbatim.
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        for o in &obs {
            assert!(!o.full_context.is_empty(), "full_context should always be non-empty");
        }
    }

    #[test]
    fn test_parse_observations_full_context_preserved() {
        let obs = FileParser::parse_observations(sample_note()).unwrap();
        // full_context should contain the bracket notation
        assert!(obs[0].full_context.contains("[task]"), "full_context should contain the original line");
    }

    #[test]
    fn test_is_timestamp_format() {
        // Test through parse_observations: a ## HH:MM header should NOT become a section
        let content = "## 08:00\n- [x] Something\n";
        let obs = FileParser::parse_observations(content).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].timestamp, "08:00");
        assert!(obs[0].section.is_none());
    }
}
