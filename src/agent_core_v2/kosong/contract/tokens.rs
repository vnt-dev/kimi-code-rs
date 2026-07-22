use super::message::{ContentPart, Message, ToolCall};
use super::tool::Tool;

// Original:
//   packages/agent-core-v2/src/kosong/contract/tokens.ts
//   estimateTokens()
pub fn estimate_tokens(text: &str) -> usize {
    let (ascii_count, non_ascii_count) =
        text.chars().fold((0usize, 0usize), |counts, character| {
            if character.is_ascii() {
                (counts.0 + 1, counts.1)
            } else {
                (counts.0, counts.1 + 1)
            }
        });
    ascii_count.div_ceil(4) + non_ascii_count
}

// Original: tokens.ts, estimateTokensForMessages()
pub fn estimate_tokens_for_messages<'a>(messages: impl IntoIterator<Item = &'a Message>) -> usize {
    messages.into_iter().map(estimate_tokens_for_message).sum()
}

// Original: tokens.ts, estimateTokensForTools()
pub fn estimate_tokens_for_tools(tools: &[Tool]) -> usize {
    tools
        .iter()
        .map(|tool| {
            estimate_tokens(&tool.name)
                + estimate_tokens(&tool.description)
                + estimate_tokens(&serde_json::to_string(&tool.parameters).unwrap())
        })
        .sum()
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/tokens.ts
//   estimateTokensForMessage()
//
// Rust adaptation:
//   Message owns an ignored OnceLock. This retains the source WeakMap's
//   observable rule: after the first call, later mutation of the same message
//   instance does not alter its estimate. Cloning creates a fresh identity.
pub fn estimate_tokens_for_message(message: &Message) -> usize {
    message.token_estimate_or_init(|| {
        estimate_tokens(message.role.as_str())
            + estimate_tokens_for_content_parts(&message.content)
            + message
                .tool_calls
                .iter()
                .map(estimate_tokens_for_tool_call)
                .sum::<usize>()
    })
}

fn estimate_tokens_for_tool_call(call: &ToolCall) -> usize {
    estimate_tokens(&call.name) + estimate_tokens(&serde_json::to_string(&call.arguments).unwrap())
}

// Original: tokens.ts, estimateTokensForContentParts()
pub fn estimate_tokens_for_content_parts(parts: &[ContentPart]) -> usize {
    parts.iter().map(estimate_tokens_for_content_part).sum()
}

pub const MEDIA_TOKEN_ESTIMATE: usize = 2_000;

// Original: tokens.ts, estimateTokensForContentPart()
pub fn estimate_tokens_for_content_part(part: &ContentPart) -> usize {
    match part {
        ContentPart::Text { text } => estimate_tokens(text),
        ContentPart::Think { think, .. } => estimate_tokens(think),
        ContentPart::ImageUrl { .. }
        | ContentPart::AudioUrl { .. }
        | ContentPart::VideoUrl { .. } => MEDIA_TOKEN_ESTIMATE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::{
        MediaUrl, Role, ToolCallType, create_user_message,
    };
    use serde_json::{Map, Value};

    #[test]
    fn estimates_ascii_at_four_characters_and_non_ascii_per_scalar() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("你好"), 2);
        assert_eq!(estimate_tokens("ab你"), 2);
        assert_eq!(estimate_tokens("😀"), 1);
    }

    #[test]
    fn message_counts_role_content_and_tool_calls() {
        let message = Message::new(
            Role::Assistant,
            vec![ContentPart::Text {
                text: "abcd".to_owned(),
            }],
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "c1".to_owned(),
                name: "tool".to_owned(),
                arguments: Some("{}".to_owned()),
                extras: None,
                stream_index: None,
            }],
        );
        let expected = estimate_tokens("assistant")
            + estimate_tokens("abcd")
            + estimate_tokens("tool")
            + estimate_tokens("\"{}\"");
        assert_eq!(estimate_tokens_for_message(&message), expected);
    }

    #[test]
    fn same_message_instance_is_memoized_after_mutation() {
        let mut message = create_user_message("hello world");
        let first = estimate_tokens_for_message(&message);
        message.content.push(ContentPart::Text {
            text: "mutated after the fact".to_owned(),
        });
        assert_eq!(estimate_tokens_for_message(&message), first);
        assert_eq!(
            estimate_tokens_for_messages([&message, &message]),
            first * 2
        );
    }

    #[test]
    fn clone_has_fresh_object_identity_and_recomputes_current_content() {
        let mut message = create_user_message("abcd");
        let before = estimate_tokens_for_message(&message);
        message.content.push(ContentPart::Text {
            text: "a much longer second part".to_owned(),
        });
        let cloned = message.clone();
        assert_eq!(estimate_tokens_for_message(&message), before);
        assert!(estimate_tokens_for_message(&cloned) > before);
    }

    #[test]
    fn media_is_flat_and_thinking_uses_text_heuristic() {
        let media = [
            ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "image://x".to_owned(),
                    id: None,
                },
            },
            ContentPart::AudioUrl {
                audio_url: MediaUrl {
                    url: "audio://x".to_owned(),
                    id: None,
                },
            },
            ContentPart::VideoUrl {
                video_url: MediaUrl {
                    url: "video://x".to_owned(),
                    id: None,
                },
            },
        ];
        for part in &media {
            assert_eq!(estimate_tokens_for_content_part(part), MEDIA_TOKEN_ESTIMATE);
        }
        assert_eq!(
            estimate_tokens_for_content_part(&ContentPart::Think {
                think: "abcd".to_owned(),
                encrypted: None,
            }),
            1
        );
    }

    #[test]
    fn tools_count_name_description_and_json_parameters() {
        let mut parameters = Map::new();
        parameters.insert("type".to_owned(), Value::String("object".to_owned()));
        let tool = Tool {
            name: "read".to_owned(),
            description: "Read a file".to_owned(),
            parameters,
            deferred: None,
        };
        let expected = estimate_tokens("read")
            + estimate_tokens("Read a file")
            + estimate_tokens("{\"type\":\"object\"}");
        assert_eq!(estimate_tokens_for_tools(&[tool]), expected);
    }
}
