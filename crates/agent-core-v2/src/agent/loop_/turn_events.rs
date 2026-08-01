//! Loop turn and streaming event payloads.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/turnEvents.ts`.

use serde::{Deserialize, Serialize};

use crate::{
    _base::errors::serialize::KimiErrorPayload,
    agent::context_memory::{
        PluginCommandTrigger, PromptOrigin, SkillActivationTrigger,
        protocol_message::MessageContent,
    },
    app::event::event_bus::DomainEventPayload,
    kosong::contract::{message::ContentPart, provider::FinishReason, usage::TokenUsage},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUserMessage {
    pub prompt_id: String,
    pub user_message_id: String,
    pub created_at: String,
    pub content: Vec<MessageContent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedEvent {
    pub turn_id: i64,
    pub origin: PromptOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_message: Option<LiveUserMessage>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndedEvent {
    pub turn_id: i64,
    pub reason: TurnEndReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KimiErrorPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepStartedEvent {
    pub turn_id: i64,
    pub step: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepCompletedEvent {
    pub turn_id: i64,
    pub step: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_first_token_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_stream_duration_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_request_build_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_server_first_token_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_server_decode_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_client_consume_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_finish_reason: Option<FinishReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_finish_reason: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepInterruptedEvent {
    pub turn_id: i64,
    pub step: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantDeltaEvent {
    pub turn_id: i64,
    pub delta: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantContentEvent {
    pub turn_id: i64,
    pub content: ContentPart,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingDeltaEvent {
    pub turn_id: i64,
    pub delta: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDeltaEvent {
    pub turn_id: i64,
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments_part: Option<String>,
}

macro_rules! impl_domain_event_payload {
    ($($event:ty => $event_type:literal),+ $(,)?) => {
        $(
            impl DomainEventPayload for $event {
                const TYPE: &'static str = $event_type;
            }
        )+
    };
}

impl_domain_event_payload!(
    TurnStartedEvent => "turn.started",
    TurnEndedEvent => "turn.ended",
    TurnStepStartedEvent => "turn.step.started",
    TurnStepCompletedEvent => "turn.step.completed",
    TurnStepInterruptedEvent => "turn.step.interrupted",
    AssistantDeltaEvent => "assistant.delta",
    AssistantContentEvent => "assistant.content",
    ThinkingDeltaEvent => "thinking.delta",
    ToolCallDeltaEvent => "tool.call.delta",
);

pub fn turn_prompt_text(input: &[ContentPart]) -> Option<String> {
    let text = input
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}
pub fn is_displayable_prompt_origin(origin: &PromptOrigin) -> bool {
    matches!(origin, PromptOrigin::User)
        || matches!(
            origin,
            PromptOrigin::SkillActivation {
                trigger: SkillActivationTrigger::UserSlash,
                ..
            } | PromptOrigin::PluginCommand {
                trigger: PluginCommandTrigger::UserSlash,
                ..
            }
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agent::context_memory::MessageContent, kosong::contract::message::MediaUrl};
    #[test]
    fn extracts_only_nonempty_text_and_limits_prompt_visibility_to_user_origins() {
        assert_eq!(
            turn_prompt_text(&[
                ContentPart::Think {
                    think: "hidden".into(),
                    encrypted: None
                },
                ContentPart::Text { text: "a".into() },
                ContentPart::ImageUrl {
                    image_url: MediaUrl {
                        url: "x".into(),
                        id: None
                    }
                },
                ContentPart::Text { text: "b".into() }
            ]),
            Some("ab".into())
        );
        assert_eq!(turn_prompt_text(&[]), None);
        assert!(is_displayable_prompt_origin(&PromptOrigin::User));
        assert!(!is_displayable_prompt_origin(
            &PromptOrigin::SystemTrigger {
                name: "goal".into()
            }
        ));
    }

    #[test]
    fn turn_started_keeps_prompt_text_and_structured_user_message() {
        let event = TurnStartedEvent {
            turn_id: 42,
            origin: PromptOrigin::User,
            prompt: Some("hello".into()),
            user_message: Some(LiveUserMessage {
                prompt_id: "prompt-1".into(),
                user_message_id: "message-1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                content: vec![MessageContent::Text {
                    text: "hello".into(),
                }],
            }),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "turnId": 42,
                "origin": { "kind": "user" },
                "prompt": "hello",
                "userMessage": {
                    "promptId": "prompt-1",
                    "userMessageId": "message-1",
                    "createdAt": "2026-01-01T00:00:00.000Z",
                    "content": [{ "type": "text", "text": "hello" }]
                }
            })
        );
    }
}
