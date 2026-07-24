//! Prompt title and last-prompt metadata helpers.
//!
//! Original: `agent/rpc/prompt-metadata.ts`.

use std::sync::LazyLock;

use regex::Regex;

use crate::{
    agent::media::extract_image_compression_captions, kosong::contract::message::ContentPart,
};

pub const MAX_TITLE_LENGTH: usize = 200;
pub const MAX_LAST_PROMPT_LENGTH: usize = 4000;

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)-----BEGIN [^-]*PRIVATE KEY-----.*?-----END [^-]*PRIVATE KEY-----")
        .expect("valid private-key pattern")
});
static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(authorization)\s*:\s*bearer\s+\S+").expect("valid authorization pattern")
});
static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(api[_-]?key|token|secret|password|passwd|pwd)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)"#).expect("valid secret pattern")
});
static OPENAI_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{12,}\b").expect("valid OpenAI-key pattern"));
static LONG_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9][A-Za-z0-9+/=_-]{39,}\b").expect("valid token pattern")
});
static CONTROL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\p{Cc}+").expect("valid control pattern"));
static WHITESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+").expect("valid whitespace pattern"));

pub fn title_from_prompt_metadata_text(text: &str) -> String {
    text.chars().take(MAX_TITLE_LENGTH).collect()
}
pub fn is_untitled(title: Option<&str>) -> bool {
    title.is_none_or(|title| title.trim().is_empty() || title == "New Session")
}
pub fn prompt_metadata_text_from_content_parts(parts: &[ContentPart]) -> Option<String> {
    let text = parts
        .iter()
        .filter_map(prompt_part_text)
        .collect::<Vec<_>>()
        .join("\n");
    sanitize_and_truncate_prompt_text(&text, MAX_LAST_PROMPT_LENGTH)
}
pub fn prompt_metadata_text_from_skill(name: &str, args: Option<&str>) -> Option<String> {
    let args = args.map(str::trim).filter(|args| !args.is_empty());
    sanitize_and_truncate_prompt_text(
        &args.map_or_else(|| format!("/{name}"), |args| format!("/{name} {args}")),
        MAX_LAST_PROMPT_LENGTH,
    )
}
pub fn prompt_metadata_text_from_plugin_command(
    plugin_id: &str,
    command_name: &str,
    args: Option<&str>,
) -> Option<String> {
    let command = format!("/{plugin_id}:{command_name}");
    let args = args.map(str::trim).filter(|args| !args.is_empty());
    sanitize_and_truncate_prompt_text(
        &args.map_or(command.clone(), |args| format!("{command} {args}")),
        MAX_LAST_PROMPT_LENGTH,
    )
}
fn prompt_part_text(part: &ContentPart) -> Option<String> {
    match part {
        ContentPart::Text { text } => {
            let text = extract_image_compression_captions(text).text;
            (!text.trim().is_empty()).then_some(text)
        }
        ContentPart::ImageUrl { .. } => Some("[image]".into()),
        ContentPart::AudioUrl { .. } => Some("[audio]".into()),
        ContentPart::VideoUrl { .. } => Some("[video]".into()),
        ContentPart::Think { .. } => None,
    }
}
pub fn sanitize_and_truncate_prompt_text(text: &str, max_length: usize) -> Option<String> {
    let text = PRIVATE_KEY.replace_all(text, "[redacted]");
    let text = AUTHORIZATION.replace_all(&text, "$1: Bearer [redacted]");
    let text = SECRET_ASSIGNMENT.replace_all(&text, "$1=[redacted]");
    let text = OPENAI_KEY.replace_all(&text, "[redacted]");
    let text = LONG_TOKEN.replace_all(&text, "[redacted]");
    let text = CONTROL.replace_all(&text, " ");
    let text = WHITESPACE.replace_all(&text, " ").trim().to_owned();
    (!text.is_empty()).then(|| text.chars().take(max_length).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_redacts_and_truncates_prompt_metadata() {
        assert_eq!(
            title_from_prompt_metadata_text(&"x".repeat(201))
                .chars()
                .count(),
            200
        );
        assert_eq!(
            sanitize_and_truncate_prompt_text(
                "Authorization: Bearer secret-value\npassword=hello",
                4000
            )
            .as_deref(),
            Some("Authorization: Bearer [redacted] password=[redacted]")
        );
        assert_eq!(
            prompt_metadata_text_from_content_parts(&[
                ContentPart::Text {
                    text: " hello ".into()
                },
                ContentPart::ImageUrl {
                    image_url: crate::kosong::contract::message::MediaUrl {
                        url: "x".into(),
                        id: None
                    }
                }
            ])
            .as_deref(),
            Some("hello [image]")
        );
        assert!(is_untitled(Some("New Session")));
    }
}
