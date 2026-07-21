use crate::tui::types::{TranscriptEntry, TranscriptEntryKind};

/// Visible text of the newest real model-authored assistant entry.
/// Synthetic assistant cards intentionally do not set `model_text`.
///
/// Original:
///   apps/kimi-code/src/tui/commands/copy.ts
///   findLastAssistantText()
pub fn find_last_assistant_text(entries: &[TranscriptEntry]) -> &str {
    entries
        .iter()
        .rev()
        .find(|entry| {
            entry.kind == TranscriptEntryKind::Assistant
                && entry.model_text == Some(true)
                && !entry.content.trim().is_empty()
        })
        .map(|entry| entry.content.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::types::TranscriptRenderMode;

    fn entry(
        id: usize,
        kind: TranscriptEntryKind,
        content: &str,
        model_text: Option<bool>,
    ) -> TranscriptEntry {
        TranscriptEntry {
            id: format!("entry-{id}"),
            kind,
            turn_id: None,
            render_mode: TranscriptRenderMode::Markdown,
            content: content.to_owned(),
            model_text,
            color: None,
            detail: None,
            bullet: None,
            compaction_data: None,
            background_agent_status: None,
            image_attachment_ids: None,
            skill_activation_id: None,
            skill_name: None,
            skill_args: None,
            skill_trigger: None,
            plugin_command_data: None,
        }
    }

    #[test]
    fn returns_newest_visible_model_reply_across_later_entries() {
        let entries = [
            entry(1, TranscriptEntryKind::Assistant, "first", Some(true)),
            entry(2, TranscriptEntryKind::User, "question", None),
            entry(3, TranscriptEntryKind::Assistant, "second", Some(true)),
            entry(4, TranscriptEntryKind::Thinking, "reasoning", None),
            entry(5, TranscriptEntryKind::Status, "Working", None),
        ];

        assert_eq!(find_last_assistant_text(&entries), "second");
    }

    #[test]
    fn skips_blank_and_synthetic_assistant_cards() {
        let entries = [
            entry(1, TranscriptEntryKind::Assistant, "real reply", Some(true)),
            entry(2, TranscriptEntryKind::Assistant, "hook result", None),
            entry(3, TranscriptEntryKind::Assistant, "  \n ", Some(true)),
        ];

        assert_eq!(find_last_assistant_text(&entries), "real reply");
        assert_eq!(find_last_assistant_text(&[]), "");
    }
}
