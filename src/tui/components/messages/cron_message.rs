use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, Spacer, Text, render::visible_width},
    theme::{ColorToken, current_theme},
    types::CronTranscriptData,
};

const STATUS_BULLET: &str = "● ";

pub struct CronMessageComponent {
    spacer: Spacer,
    data: CronTranscriptData,
    title: &'static str,
    detail: Option<String>,
    prompt_text: Text,
    prompt: String,
}

impl CronMessageComponent {
    pub fn new(prompt: impl Into<String>, data: CronTranscriptData) -> Self {
        let prompt = prompt.into();
        let title = if data.missed_count.is_some() {
            "Missed scheduled reminders"
        } else {
            "Scheduled reminder fired"
        };
        let detail = cron_detail(&data);
        let prompt_text = Text::new(current_theme().fg(ColorToken::Text, &prompt), 0, 0);
        Self {
            spacer: Spacer::new(1),
            data,
            title,
            detail,
            prompt_text,
            prompt,
        }
    }
}

impl Component for CronMessageComponent {
    /// Original: cron-message.ts CronMessageComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }

        let missed = self.data.missed_count.is_some();
        let title_token = if self.data.stale == Some(true) || missed {
            ColorToken::Warning
        } else {
            ColorToken::Accent
        };
        let theme = current_theme();
        let bullet = theme.bold_fg(title_token, STATUS_BULLET);
        let bullet_width = visible_width(&bullet);
        let content_width = width.saturating_sub(bullet_width).max(1);
        let continuation_indent = " ".repeat(bullet_width);
        let mut lines = self.spacer.render(width);

        let mut title_text = Text::new(theme.bold_fg(title_token, self.title), 0, 0);
        for (index, line) in title_text.render(content_width).into_iter().enumerate() {
            lines.push(format!(
                "{}{line}",
                if index == 0 {
                    &bullet
                } else {
                    &continuation_indent
                }
            ));
        }
        if let Some(detail) = &self.detail {
            let mut detail_text = Text::new(theme.fg(ColorToken::TextDim, detail), 0, 0);
            for line in detail_text.render(content_width) {
                lines.push(format!("{continuation_indent}{line}"));
            }
        }
        for line in self.prompt_text.render(content_width) {
            lines.push(format!("{continuation_indent}{line}"));
        }
        lines
    }

    fn invalidate(&mut self) {
        self.prompt_text
            .set_text(current_theme().fg(ColorToken::Text, &self.prompt));
        self.prompt_text.invalidate();
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::CronMessage
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Original: cron-message.ts cronDetail()
pub fn cron_detail(data: &CronTranscriptData) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(cron) = data.cron.as_deref().filter(|value| !value.is_empty()) {
        parts.push(cron.to_owned());
    }
    if let Some(job_id) = data.job_id.as_deref().filter(|value| !value.is_empty()) {
        parts.push(format!("job {job_id}"));
    }
    if data.recurring == Some(false) {
        parts.push("one-shot".to_owned());
    }
    if let Some(count) = data.coalesced_count.filter(|count| *count > 1) {
        parts.push(format!("{count} fires coalesced"));
    }
    if let Some(count) = data.missed_count {
        parts.push(format!("{count} missed"));
    }
    if data.stale == Some(true) {
        parts.push("final delivery".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut index = 0;
        while index < text.len() {
            if text.as_bytes()[index] == 0x1b && text.as_bytes().get(index + 1) == Some(&b'[') {
                index += 2;
                while index < text.len() && text.as_bytes()[index] != b'm' {
                    index += 1;
                }
                index = (index + 1).min(text.len());
            } else if let Some(character) = text[index..].chars().next() {
                output.push(character);
                index += character.len_utf8();
            } else {
                break;
            }
        }
        output
    }

    fn reminder_data() -> CronTranscriptData {
        CronTranscriptData {
            job_id: Some("job-with-a-very-long-identifier-for-width-testing".to_owned()),
            cron: Some("*/15 * * * *".to_owned()),
            recurring: Some(true),
            coalesced_count: None,
            stale: Some(true),
            missed_count: Some(3),
        }
    }

    #[test]
    fn builds_detail_in_original_order() {
        let data = CronTranscriptData {
            job_id: Some("daily".to_owned()),
            cron: Some("0 9 * * *".to_owned()),
            recurring: Some(false),
            coalesced_count: Some(4),
            stale: Some(true),
            missed_count: Some(2),
        };
        assert_eq!(
            cron_detail(&data).as_deref(),
            Some(
                "0 9 * * * | job daily | one-shot | 4 fires coalesced | 2 missed | final delivery"
            )
        );
        assert_eq!(cron_detail(&CronTranscriptData::default()), None);
    }

    #[test]
    fn renders_title_detail_and_prompt_with_cron_role() {
        let mut component = CronMessageComponent::new("Check the payload", reminder_data());
        let rendered = component.render(80);
        let plain = rendered
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert_eq!(plain[0], "");
        assert!(plain[1].contains("Missed scheduled reminders"));
        let combined = plain
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(combined.contains("3 missed"));
        assert!(plain.iter().any(|line| line.contains("Check the payload")));
        assert_eq!(component.role(), ComponentRole::CronMessage);
    }

    #[test]
    fn keeps_content_within_tested_narrow_widths() {
        let mut component = CronMessageComponent::new(
            "Please investigate the reminder payload and report back.",
            reminder_data(),
        );
        for width in [39, 20, 10, 4] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }

    #[test]
    fn serializes_camel_case_cron_fields() {
        let value = serde_json::to_value(reminder_data()).unwrap_or_default();
        assert_eq!(
            value["jobId"],
            "job-with-a-very-long-identifier-for-width-testing"
        );
        assert_eq!(value["missedCount"], 3);
        assert!(value.get("coalescedCount").is_none());
    }
}
