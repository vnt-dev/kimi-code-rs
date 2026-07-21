use crate::tui::types::{
    BackgroundAgentMetadata, BackgroundAgentStatusData, BackgroundAgentStatusPhase,
};

const MAX_BACKGROUND_FIELD_LENGTH: usize = 240;

fn normalize_background_field(value: Option<&str>) -> Option<String> {
    let collapsed = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }

    if collapsed.chars().count() <= MAX_BACKGROUND_FIELD_LENGTH {
        return Some(collapsed);
    }

    let prefix = collapsed
        .chars()
        .take(MAX_BACKGROUND_FIELD_LENGTH - 3)
        .collect::<String>();
    Some(format!("{prefix}..."))
}

/// Optional terminal data supplied when a background agent finishes.
/// `result_summary` is intentionally retained even though the original
/// formatter currently only appends errors for failed agents.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundAgentTranscriptExtras {
    pub result_summary: Option<String>,
    pub error: Option<String>,
}

/// Original:
///   apps/kimi-code/src/tui/utils/background-agent-status.ts
///   formatBackgroundAgentTranscript()
pub fn format_background_agent_transcript(
    phase: BackgroundAgentStatusPhase,
    meta: &BackgroundAgentMetadata,
    extras: Option<&BackgroundAgentTranscriptExtras>,
) -> BackgroundAgentStatusData {
    let normalized_agent_name = normalize_background_field(meta.agent_name.as_deref());
    let subject = normalized_agent_name
        .map(|name| format!("{name} agent"))
        .unwrap_or_else(|| "agent".to_owned());
    let headline = match phase {
        BackgroundAgentStatusPhase::Started => format!("{subject} started in background"),
        BackgroundAgentStatusPhase::Completed => format!("{subject} completed in background"),
        BackgroundAgentStatusPhase::Failed => format!("{subject} failed in background"),
    };

    let tail = (phase == BackgroundAgentStatusPhase::Failed)
        .then(|| extras.and_then(|value| value.error.as_deref()))
        .flatten()
        .and_then(|value| normalize_background_field(Some(value)));
    let detail = [
        normalize_background_field(meta.description.as_deref()),
        tail,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    BackgroundAgentStatusData {
        phase,
        headline,
        detail: (!detail.is_empty()).then(|| detail.join(" · ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> BackgroundAgentMetadata {
        BackgroundAgentMetadata {
            agent_id: "agent-child".to_owned(),
            parent_tool_call_id: "tool-parent".to_owned(),
            agent_name: Some("  explore\n agent  ".to_owned()),
            description: Some(" Explore   project\tstructure ".to_owned()),
        }
    }

    #[test]
    fn formats_started_agent_and_collapses_whitespace() {
        let data = format_background_agent_transcript(
            BackgroundAgentStatusPhase::Started,
            &metadata(),
            None,
        );

        assert_eq!(data.phase, BackgroundAgentStatusPhase::Started);
        assert_eq!(data.headline, "explore agent agent started in background");
        assert_eq!(data.detail.as_deref(), Some("Explore project structure"));
    }

    #[test]
    fn failed_agent_appends_error_but_completed_agent_does_not() {
        let extras = BackgroundAgentTranscriptExtras {
            result_summary: Some("unused result".to_owned()),
            error: Some(" network   unavailable ".to_owned()),
        };

        let failed = format_background_agent_transcript(
            BackgroundAgentStatusPhase::Failed,
            &metadata(),
            Some(&extras),
        );
        let completed = format_background_agent_transcript(
            BackgroundAgentStatusPhase::Completed,
            &metadata(),
            Some(&extras),
        );

        assert_eq!(
            failed.detail.as_deref(),
            Some("Explore project structure · network unavailable")
        );
        assert_eq!(
            completed.detail.as_deref(),
            Some("Explore project structure")
        );
    }

    #[test]
    fn uses_generic_subject_and_drops_empty_detail() {
        let meta = BackgroundAgentMetadata {
            agent_id: "agent-child".to_owned(),
            parent_tool_call_id: "tool-parent".to_owned(),
            agent_name: Some("  \n ".to_owned()),
            description: Some("\t".to_owned()),
        };

        let data =
            format_background_agent_transcript(BackgroundAgentStatusPhase::Completed, &meta, None);

        assert_eq!(data.headline, "agent completed in background");
        assert_eq!(data.detail, None);
    }

    #[test]
    fn truncates_normalized_fields_to_240_characters() {
        let meta = BackgroundAgentMetadata {
            agent_id: "agent-child".to_owned(),
            parent_tool_call_id: "tool-parent".to_owned(),
            agent_name: None,
            description: Some("x".repeat(241)),
        };

        let data =
            format_background_agent_transcript(BackgroundAgentStatusPhase::Started, &meta, None);
        let detail = data.detail.as_deref().unwrap_or_default();

        assert_eq!(detail.chars().count(), 240);
        assert!(detail.ends_with("..."));
    }

    #[test]
    fn keeps_fields_at_or_below_the_limit_unchanged() {
        for length in [237, 238, 239, 240] {
            let value = "x".repeat(length);
            assert_eq!(
                normalize_background_field(Some(&value)).as_deref(),
                Some(value.as_str())
            );
        }
    }
}
