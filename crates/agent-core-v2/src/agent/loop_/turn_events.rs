//! Loop turn and streaming event payloads.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/turnEvents.ts`.

use serde::{Deserialize, Serialize};

use crate::{
    _base::errors::serialize::KimiErrorPayload,
    agent::context_memory::{PluginCommandTrigger, PromptOrigin, SkillActivationTrigger},
    kosong::contract::{message::ContentPart, provider::FinishReason, usage::TokenUsage},
};

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
    use crate::kosong::contract::message::MediaUrl;
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
}
