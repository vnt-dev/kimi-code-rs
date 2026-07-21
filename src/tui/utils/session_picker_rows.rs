use serde_json::{Map, Value};

use crate::sdk::types::SessionSummary;

/// Session data consumed by the session-picker component.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub work_dir: String,
    pub updated_at: f64,
    pub metadata: Option<Map<String, Value>>,
}

// Original:
//   apps/kimi-code/src/tui/utils/session-picker-rows.ts
//   sessionRowsForPicker()
pub fn session_rows_for_picker(
    sessions: &[SessionSummary],
    current_session_id: &str,
    current_session_has_content: bool,
) -> Vec<SessionRow> {
    sessions
        .iter()
        .filter(|session| current_session_has_content || session.id != current_session_id)
        .map(|session| SessionRow {
            id: session.id.clone(),
            title: session.title.clone(),
            last_prompt: session.last_prompt.clone(),
            work_dir: session.work_dir.clone(),
            updated_at: session.updated_at.or(session.created_at).unwrap_or(0.0),
            metadata: session.metadata.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::session_rows_for_picker;
    use crate::sdk::types::SessionSummary;

    fn summary(id: &str, title: Option<&str>, last_prompt: Option<&str>) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            title: title.map(str::to_owned),
            last_prompt: last_prompt.map(str::to_owned),
            work_dir: "/tmp/project".to_owned(),
            session_dir: format!("/tmp/home/sessions/{id}"),
            created_at: Some(1.0),
            updated_at: Some(2.0),
            archived: None,
            metadata: None,
            additional_dirs: None,
        }
    }

    #[test]
    fn omits_the_current_session_when_it_has_no_content() {
        let rows = session_rows_for_picker(
            &[
                summary("ses_current", Some("New Session"), None),
                summary("ses_previous", Some("New Session"), None),
            ],
            "ses_current",
            false,
        );

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["ses_previous"]
        );
    }

    #[test]
    fn keeps_the_current_session_when_it_has_content() {
        let rows = session_rows_for_picker(
            &[summary(
                "ses_current",
                Some("Implement feature"),
                Some("Implement feature"),
            )],
            "ses_current",
            true,
        );

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["ses_current"]
        );
    }

    #[test]
    fn does_not_filter_empty_historical_sessions() {
        let rows = session_rows_for_picker(
            &[
                summary("ses_current", Some("New Session"), None),
                summary("ses_previous_empty", Some("New Session"), None),
            ],
            "ses_current",
            false,
        );

        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["ses_previous_empty"]
        );
    }

    #[test]
    fn maps_fields_and_preserves_timestamp_fallbacks() {
        let mut current = summary("ses_current", None, None);
        current.updated_at = None;
        current.created_at = None;
        let rows = session_rows_for_picker(&[current], "other", false);
        assert_eq!(rows[0].title, None);
        assert_eq!(rows[0].last_prompt, None);
        assert_eq!(rows[0].work_dir, "/tmp/project");
        assert_eq!(rows[0].updated_at, 0.0);
    }
}
