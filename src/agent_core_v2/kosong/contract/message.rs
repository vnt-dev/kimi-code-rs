use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::tool::Tool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   ContentPart
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "think")]
    Think {
        think: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted: Option<String>,
    },
    #[serde(rename = "image_url")]
    ImageUrl {
        #[serde(rename = "imageUrl")]
        image_url: MediaUrl,
    },
    #[serde(rename = "audio_url")]
    AudioUrl {
        #[serde(rename = "audioUrl")]
        audio_url: MediaUrl,
    },
    #[serde(rename = "video_url")]
    VideoUrl {
        #[serde(rename = "videoUrl")]
        video_url: MediaUrl,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallType {
    #[serde(rename = "function")]
    Function,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamIndex {
    Number(i64),
    String(String),
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   ToolCall
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    #[serde(rename = "type")]
    pub call_type: ToolCallType,
    pub id: String,
    pub name: String,
    pub arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extras: Option<Map<String, Value>>,
    #[serde(rename = "_streamIndex", skip_serializing_if = "Option::is_none")]
    pub stream_index: Option<StreamIndex>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolCallPartType {
    #[serde(rename = "tool_call_part")]
    ToolCallPart,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   ToolCallPart
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallPart {
    #[serde(rename = "type")]
    pub part_type: ToolCallPartType,
    pub arguments_part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<StreamIndex>,
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   StreamedMessagePart
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamedMessagePart {
    Content(ContentPart),
    ToolCall(ToolCall),
    ToolCallPart(ToolCallPart),
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   Message
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: Vec<ContentPart>,
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

// Original: message.ts, isContentPart()
pub fn is_content_part(part: &StreamedMessagePart) -> bool {
    matches!(part, StreamedMessagePart::Content(_))
}

// Original: message.ts, isToolDeclarationOnlyMessage()
pub fn is_tool_declaration_only_message(message: &Message) -> bool {
    message
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
        && message.content.is_empty()
        && message.tool_calls.is_empty()
}

// Original: message.ts, isToolCall()
pub fn is_tool_call(part: &StreamedMessagePart) -> bool {
    matches!(part, StreamedMessagePart::ToolCall(_))
}

// Original: message.ts, isToolCallPart()
pub fn is_tool_call_part(part: &StreamedMessagePart) -> bool {
    matches!(part, StreamedMessagePart::ToolCallPart(_))
}

// Original:
//   packages/agent-core-v2/src/kosong/contract/message.ts
//   mergeInPlace()
pub fn merge_in_place(target: &mut StreamedMessagePart, source: &StreamedMessagePart) -> bool {
    match (target, source) {
        (
            StreamedMessagePart::Content(ContentPart::Text { text: target }),
            StreamedMessagePart::Content(ContentPart::Text { text: source }),
        ) => {
            target.push_str(source);
            true
        }
        (
            StreamedMessagePart::Content(ContentPart::Think {
                think: target,
                encrypted: target_encrypted,
            }),
            StreamedMessagePart::Content(ContentPart::Think {
                think: source,
                encrypted: source_encrypted,
            }),
        ) => {
            if target_encrypted.is_some() {
                return false;
            }
            target.push_str(source);
            if let Some(encrypted) = source_encrypted {
                *target_encrypted = Some(encrypted.clone());
            }
            true
        }
        (
            StreamedMessagePart::ToolCall(ToolCall {
                arguments: target, ..
            }),
            StreamedMessagePart::ToolCallPart(ToolCallPart {
                arguments_part: source,
                ..
            }),
        ) => {
            if let Some(source) = source {
                if let Some(target) = target {
                    target.push_str(source);
                } else {
                    *target = Some(source.clone());
                }
            }
            true
        }
        _ => false,
    }
}

// Original: message.ts, extractText()
pub fn extract_text(message: &Message, separator: &str) -> String {
    message
        .content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(separator)
}

// Original: message.ts, getTextContent()
pub fn get_text_content(message: &Message) -> String {
    extract_text(message, "")
}

// Original: message.ts, createUserMessage()
pub fn create_user_message(content: impl Into<String>) -> Message {
    Message {
        role: Role::User,
        name: None,
        content: vec![ContentPart::Text {
            text: content.into(),
        }],
        tool_calls: Vec::new(),
        tool_call_id: None,
        partial: None,
        tools: None,
    }
}

// Original: message.ts, createAssistantMessage()
pub fn create_assistant_message(
    content: Vec<ContentPart>,
    tool_calls: Option<Vec<ToolCall>>,
) -> Message {
    Message {
        role: Role::Assistant,
        name: None,
        content,
        tool_calls: tool_calls.unwrap_or_default(),
        tool_call_id: None,
        partial: None,
        tools: None,
    }
}

pub enum ToolOutput {
    Text(String),
    Parts(Vec<ContentPart>),
}

// Original: message.ts, createToolMessage()
pub fn create_tool_message(tool_call_id: impl Into<String>, output: ToolOutput) -> Message {
    let content = match output {
        ToolOutput::Text(text) => vec![ContentPart::Text { text }],
        ToolOutput::Parts(parts) => parts,
    };
    Message {
        role: Role::Tool,
        name: None,
        content,
        tool_calls: Vec::new(),
        tool_call_id: Some(tool_call_id.into()),
        partial: None,
        tools: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function_call(arguments: Option<&str>) -> ToolCall {
        ToolCall {
            call_type: ToolCallType::Function,
            id: "c1".to_owned(),
            name: "read".to_owned(),
            arguments: arguments.map(str::to_owned),
            extras: None,
            stream_index: Some(StreamIndex::Number(2)),
        }
    }

    #[test]
    fn message_serialization_preserves_discriminants_and_camel_case_fields() {
        let message = Message {
            role: Role::Assistant,
            name: None,
            content: vec![
                ContentPart::Text {
                    text: "answer".to_owned(),
                },
                ContentPart::Think {
                    think: "reason".to_owned(),
                    encrypted: Some("cipher".to_owned()),
                },
                ContentPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "https://example.test/image.png".to_owned(),
                        id: Some("image-1".to_owned()),
                    },
                },
            ],
            tool_calls: vec![function_call(Some("{}"))],
            tool_call_id: None,
            partial: Some(true),
            tools: None,
        };
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["role"], "assistant");
        assert_eq!(
            value["content"][0],
            serde_json::json!({"type":"text","text":"answer"})
        );
        assert_eq!(
            value["content"][1],
            serde_json::json!({"type":"think","think":"reason","encrypted":"cipher"})
        );
        assert_eq!(value["content"][2]["type"], "image_url");
        assert_eq!(value["content"][2]["imageUrl"]["id"], "image-1");
        assert_eq!(value["toolCalls"][0]["type"], "function");
        assert_eq!(value["toolCalls"][0]["_streamIndex"], 2);
        assert_eq!(value["partial"], true);
        assert!(value.get("name").is_none());
    }

    #[test]
    fn type_guards_and_tool_declaration_predicate_match_variants() {
        let content = StreamedMessagePart::Content(ContentPart::Text {
            text: "x".to_owned(),
        });
        let call = StreamedMessagePart::ToolCall(function_call(None));
        let delta = StreamedMessagePart::ToolCallPart(ToolCallPart {
            part_type: ToolCallPartType::ToolCallPart,
            arguments_part: None,
            index: None,
        });
        assert!(is_content_part(&content));
        assert!(is_tool_call(&call));
        assert!(is_tool_call_part(&delta));

        let mut message = create_assistant_message(Vec::new(), None);
        message.tools = Some(vec![Tool {
            name: "read".to_owned(),
            description: "Read".to_owned(),
            parameters: Map::new(),
            deferred: None,
        }]);
        assert!(is_tool_declaration_only_message(&message));
        message.content.push(ContentPart::Text {
            text: String::new(),
        });
        assert!(!is_tool_declaration_only_message(&message));
    }

    #[test]
    fn merge_in_place_preserves_all_original_branch_rules() {
        let mut text = StreamedMessagePart::Content(ContentPart::Text {
            text: "a".to_owned(),
        });
        assert!(merge_in_place(
            &mut text,
            &StreamedMessagePart::Content(ContentPart::Text {
                text: "b".to_owned(),
            })
        ));
        assert_eq!(
            text,
            StreamedMessagePart::Content(ContentPart::Text {
                text: "ab".to_owned()
            })
        );

        let mut think = StreamedMessagePart::Content(ContentPart::Think {
            think: "a".to_owned(),
            encrypted: None,
        });
        assert!(merge_in_place(
            &mut think,
            &StreamedMessagePart::Content(ContentPart::Think {
                think: "b".to_owned(),
                encrypted: Some("cipher".to_owned()),
            })
        ));
        assert!(!merge_in_place(
            &mut think,
            &StreamedMessagePart::Content(ContentPart::Think {
                think: "ignored".to_owned(),
                encrypted: None,
            })
        ));

        let mut call = StreamedMessagePart::ToolCall(function_call(None));
        assert!(merge_in_place(
            &mut call,
            &StreamedMessagePart::ToolCallPart(ToolCallPart {
                part_type: ToolCallPartType::ToolCallPart,
                arguments_part: Some("{\"path\":".to_owned()),
                index: None,
            })
        ));
        assert!(merge_in_place(
            &mut call,
            &StreamedMessagePart::ToolCallPart(ToolCallPart {
                part_type: ToolCallPartType::ToolCallPart,
                arguments_part: Some("\"a\"}".to_owned()),
                index: None,
            })
        ));
        let StreamedMessagePart::ToolCall(call) = call else {
            panic!("expected tool call")
        };
        assert_eq!(call.arguments.as_deref(), Some("{\"path\":\"a\"}"));
    }

    #[test]
    fn text_extraction_ignores_non_text_content() {
        let message = create_assistant_message(
            vec![
                ContentPart::Text {
                    text: "a".to_owned(),
                },
                ContentPart::Think {
                    think: "hidden".to_owned(),
                    encrypted: None,
                },
                ContentPart::Text {
                    text: "b".to_owned(),
                },
            ],
            None,
        );
        assert_eq!(extract_text(&message, "\n"), "a\nb");
        assert_eq!(get_text_content(&message), "ab");
    }

    #[test]
    fn constructors_preserve_source_defaults_and_output_forms() {
        let user = create_user_message("hello");
        assert_eq!(user.role, Role::User);
        assert!(user.tool_calls.is_empty());
        assert_eq!(get_text_content(&user), "hello");

        let text_tool = create_tool_message("c1", ToolOutput::Text("done".to_owned()));
        assert_eq!(text_tool.role, Role::Tool);
        assert_eq!(text_tool.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(get_text_content(&text_tool), "done");

        let media_tool = create_tool_message(
            "c2",
            ToolOutput::Parts(vec![ContentPart::AudioUrl {
                audio_url: MediaUrl {
                    url: "audio://result".to_owned(),
                    id: None,
                },
            }]),
        );
        assert!(matches!(
            media_tool.content[0],
            ContentPart::AudioUrl { .. }
        ));
    }
}
