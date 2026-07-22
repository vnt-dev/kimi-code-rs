use std::collections::HashMap;
use uuid::Uuid;

use crate::kosong::contract::message::{
    StreamIndex, StreamedMessagePart, ToolCall, ToolCallPart, ToolCallPartType, ToolCallType,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatCompletionStreamToolFunctionDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChatCompletionStreamToolCallDelta {
    pub index: Option<StreamIndex>,
    pub id: Option<String>,
    pub function: Option<ChatCompletionStreamToolFunctionDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BufferedChatCompletionToolCall {
    pub id: Option<String>,
    pub arguments: String,
    pub emitted: bool,
}

fn random_tool_call_id() -> String {
    Uuid::new_v4().to_string()
}

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/openai/chat-completions-stream.ts
//   convertChatCompletionStreamToolCall()
pub fn convert_chat_completion_stream_tool_call(
    tool_call: &ChatCompletionStreamToolCallDelta,
    buffered_by_index: &mut HashMap<StreamIndex, BufferedChatCompletionToolCall>,
) -> Vec<StreamedMessagePart> {
    let Some(function) = tool_call.function.as_ref() else {
        return Vec::new();
    };
    let concrete_name = function.name.as_deref().filter(|name| !name.is_empty());
    let arguments = function
        .arguments
        .as_deref()
        .filter(|arguments| !arguments.is_empty());

    let Some(stream_index) = tool_call.index.as_ref() else {
        if let Some(name) = concrete_name {
            return vec![StreamedMessagePart::ToolCall(ToolCall {
                call_type: ToolCallType::Function,
                id: tool_call.id.clone().unwrap_or_else(random_tool_call_id),
                name: name.to_owned(),
                arguments: function.arguments.clone(),
                extras: None,
                stream_index: None,
            })];
        }
        return arguments.map_or_else(Vec::new, |arguments| {
            vec![StreamedMessagePart::ToolCallPart(ToolCallPart {
                part_type: ToolCallPartType::ToolCallPart,
                arguments_part: Some(arguments.to_owned()),
                index: None,
            })]
        });
    };

    let buffered = buffered_by_index.entry(stream_index.clone()).or_default();
    if let Some(id) = tool_call.id.as_ref() {
        buffered.id = Some(id.clone());
    }

    if !buffered.emitted {
        let Some(name) = concrete_name else {
            if let Some(arguments) = arguments {
                buffered.arguments.push_str(arguments);
            }
            return Vec::new();
        };
        buffered.emitted = true;
        let initial_arguments = if buffered.arguments.is_empty() {
            function.arguments.clone()
        } else {
            buffered.arguments.push_str(arguments.unwrap_or_default());
            Some(std::mem::take(&mut buffered.arguments))
        };
        buffered.arguments.clear();
        return vec![StreamedMessagePart::ToolCall(ToolCall {
            call_type: ToolCallType::Function,
            id: buffered
                .id
                .clone()
                .or_else(|| tool_call.id.clone())
                .unwrap_or_else(random_tool_call_id),
            name: name.to_owned(),
            arguments: initial_arguments,
            extras: None,
            stream_index: Some(stream_index.clone()),
        })];
    }

    arguments.map_or_else(Vec::new, |arguments| {
        vec![StreamedMessagePart::ToolCallPart(ToolCallPart {
            part_type: ToolCallPartType::ToolCallPart,
            arguments_part: Some(arguments.to_owned()),
            index: Some(stream_index.clone()),
        })]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(
        index: Option<StreamIndex>,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) -> ChatCompletionStreamToolCallDelta {
        ChatCompletionStreamToolCallDelta {
            index,
            id: id.map(str::to_owned),
            function: Some(ChatCompletionStreamToolFunctionDelta {
                name: name.map(str::to_owned),
                arguments: arguments.map(str::to_owned),
            }),
        }
    }

    #[test]
    fn ignores_missing_function_and_empty_deltas() {
        let mut buffered = HashMap::new();
        assert!(
            convert_chat_completion_stream_tool_call(
                &ChatCompletionStreamToolCallDelta::default(),
                &mut buffered
            )
            .is_empty()
        );
        assert!(
            convert_chat_completion_stream_tool_call(
                &delta(None, None, Some(""), Some("")),
                &mut buffered
            )
            .is_empty()
        );
    }

    #[test]
    fn unindexed_name_or_arguments_emit_immediately() {
        let mut buffered = HashMap::new();
        let output = convert_chat_completion_stream_tool_call(
            &delta(None, Some("call-1"), Some("read"), Some("")),
            &mut buffered,
        );
        let StreamedMessagePart::ToolCall(call) = &output[0] else {
            panic!("expected header")
        };
        assert_eq!(call.id, "call-1");
        assert_eq!(call.name, "read");
        assert_eq!(call.arguments.as_deref(), Some(""));

        let output = convert_chat_completion_stream_tool_call(
            &delta(None, None, None, Some("{\"path\":")),
            &mut buffered,
        );
        let StreamedMessagePart::ToolCallPart(part) = &output[0] else {
            panic!("expected arguments")
        };
        assert_eq!(part.arguments_part.as_deref(), Some("{\"path\":"));
        assert_eq!(part.index, None);
    }

    #[test]
    fn indexed_arguments_buffer_until_name_then_emit_header_once() {
        let index = StreamIndex::Number(2);
        let mut buffered = HashMap::new();
        assert!(
            convert_chat_completion_stream_tool_call(
                &delta(Some(index.clone()), Some("call-2"), None, Some("{\"a\":")),
                &mut buffered
            )
            .is_empty()
        );
        let output = convert_chat_completion_stream_tool_call(
            &delta(Some(index.clone()), None, Some("run"), Some("1}")),
            &mut buffered,
        );
        let StreamedMessagePart::ToolCall(call) = &output[0] else {
            panic!("expected header")
        };
        assert_eq!(call.id, "call-2");
        assert_eq!(call.arguments.as_deref(), Some("{\"a\":1}"));
        assert_eq!(call.stream_index, Some(index.clone()));
        assert!(buffered[&index].emitted);
        assert!(buffered[&index].arguments.is_empty());

        let output = convert_chat_completion_stream_tool_call(
            &delta(Some(index.clone()), None, Some("ignored"), Some(" more")),
            &mut buffered,
        );
        let StreamedMessagePart::ToolCallPart(part) = &output[0] else {
            panic!("expected part")
        };
        assert_eq!(part.arguments_part.as_deref(), Some(" more"));
        assert_eq!(part.index, Some(index));
    }

    #[test]
    fn generated_fallback_id_is_a_uuid() {
        let mut buffered = HashMap::new();
        let output = convert_chat_completion_stream_tool_call(
            &delta(None, None, Some("read"), None),
            &mut buffered,
        );
        let StreamedMessagePart::ToolCall(call) = &output[0] else {
            panic!("expected header")
        };
        assert!(Uuid::parse_str(&call.id).is_ok());
    }
}
