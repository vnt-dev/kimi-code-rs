//! Tool result normalization.
//!
//! Original: `toolExecutorService.ts`, `normalizeToolResult()`.

use crate::{
    kosong::contract::message::{ContentPart, content_text_parts},
    tool::{ExecutableToolOutput, ExecutableToolResult, ToolResult},
};

pub const TOOL_OUTPUT_EMPTY: &str = "Tool output is empty.";
pub const TOOL_OUTPUT_NON_TEXT: &str = "Tool returned non-text content.";

pub fn normalize_tool_result(result: ExecutableToolResult) -> ToolResult {
    let output = match result.output {
        ExecutableToolOutput::Text(text) if text.is_empty() => {
            ExecutableToolOutput::Text(TOOL_OUTPUT_EMPTY.into())
        }
        ExecutableToolOutput::Text(text) => ExecutableToolOutput::Text(text),
        ExecutableToolOutput::Content(parts) if parts.is_empty() => {
            ExecutableToolOutput::Text(TOOL_OUTPUT_EMPTY.into())
        }
        ExecutableToolOutput::Content(mut parts) => {
            let has_media = parts.iter().any(|part| {
                matches!(
                    part,
                    ContentPart::ImageUrl { .. }
                        | ContentPart::AudioUrl { .. }
                        | ContentPart::VideoUrl { .. }
                )
            });
            if has_media {
                if !parts
                    .iter()
                    .any(|part| matches!(part, ContentPart::Text { text } if !text.is_empty()))
                {
                    parts.insert(
                        0,
                        ContentPart::Text {
                            text: TOOL_OUTPUT_NON_TEXT.into(),
                        },
                    );
                }
                ExecutableToolOutput::Content(parts)
            } else {
                let text = content_text_parts(&parts);
                ExecutableToolOutput::Text(if text.is_empty() {
                    TOOL_OUTPUT_EMPTY.into()
                } else {
                    text
                })
            }
        }
    };
    ToolResult {
        output,
        is_error: result.is_error,
        stop_turn: result.stop_turn,
        truncated: result.truncated.filter(|value| *value),
        note: result.note.filter(|note| !note.is_empty()),
        delivery: None,
        description: None,
        display: None,
        approval_rule: None,
        stop_batch_after_this: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    #[test]
    fn normalizes_empty_and_media_only_outputs_like_source() {
        assert_eq!(
            normalize_tool_result(ExecutableToolResult::success("".to_owned())).output,
            ExecutableToolOutput::Text(TOOL_OUTPUT_EMPTY.into())
        );
        let media = normalize_tool_result(ExecutableToolResult::success(
            ExecutableToolOutput::Content(vec![ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "data:image/png;base64,AA==".into(),
                    id: None,
                },
            }]),
        ));
        assert_eq!(
            media.output,
            ExecutableToolOutput::Content(vec![
                ContentPart::Text {
                    text: TOOL_OUTPUT_NON_TEXT.into()
                },
                ContentPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "data:image/png;base64,AA==".into(),
                        id: None
                    }
                },
            ])
        );
    }
}
