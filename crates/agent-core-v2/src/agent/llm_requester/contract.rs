//! Agent-scoped LLM requester contract.
//!
//! Original: `packages/agent-core-v2/src/agent/llmRequester/llmRequester.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use crate::{
    _base::{di::instantiation::ServiceIdentifier, utils::abort::AbortSignal},
    kosong::{
        contract::{
            message::{Message, StreamedMessagePart},
            provider::{FinishReason, ThinkingEffort},
            request_trace::LlmRequestTrace,
            tool::Tool,
            usage::TokenUsage,
        },
        model::ModelRequestTiming,
    },
};

use super::AgentLlmRequestSource;

pub type AgentLlmRequestPartHandler =
    Arc<dyn Fn(StreamedMessagePart) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;
pub type AgentLlmRequestError = Arc<dyn Error + Send + Sync>;
#[derive(Clone, Debug)]
pub struct AgentLlmRequestFinish {
    pub message: Message,
    pub usage: TokenUsage,
    pub model: Option<String>,
    pub provider_finish_reason: Option<FinishReason>,
    pub raw_finish_reason: Option<String>,
    pub provider_message_id: Option<String>,
    pub timing: Option<ModelRequestTiming>,
    pub trace_id: Option<String>,
}
#[derive(Clone, Default)]
pub struct AgentLlmRequestOverrides {
    pub messages: Option<Vec<Message>>,
    pub tools: Option<Vec<Tool>>,
    pub system_prompt: Option<String>,
    pub source: Option<AgentLlmRequestSource>,
    pub max_output_size: Option<f64>,
}
pub struct AgentLlmRequestTask {
    pub trace: LlmRequestTrace,
    pub result: BoxFuture<'static, Result<AgentLlmRequestFinish, AgentLlmRequestError>>,
}
#[derive(Clone, Debug)]
pub struct PreparedTurnRequestConfig {
    pub thinking_effort: ThinkingEffort,
}

#[async_trait]
pub trait AgentLlmRequesterServiceContract: Send + Sync {
    fn prepare_turn_config(&self, turn_id: i64) -> Option<PreparedTurnRequestConfig>;
    async fn request(
        &self,
        overrides: Option<AgentLlmRequestOverrides>,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
    ) -> Result<AgentLlmRequestFinish, AgentLlmRequestError>;
    fn start(
        &self,
        overrides: Option<AgentLlmRequestOverrides>,
        on_part: Option<AgentLlmRequestPartHandler>,
        signal: Option<AbortSignal>,
    ) -> AgentLlmRequestTask;
}
#[derive(Clone)]
pub struct AgentLlmRequesterServiceHandle(pub Arc<dyn AgentLlmRequesterServiceContract>);
impl Deref for AgentLlmRequesterServiceHandle {
    type Target = dyn AgentLlmRequesterServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const AGENT_LLM_REQUESTER_SERVICE_ID: ServiceIdentifier<AgentLlmRequesterServiceHandle> =
    ServiceIdentifier::new("agentLLMRequesterService");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_identity_and_optional_override_defaults_match_source() {
        assert_eq!(
            AGENT_LLM_REQUESTER_SERVICE_ID.to_string(),
            "agentLLMRequesterService"
        );
        let options = AgentLlmRequestOverrides::default();
        assert!(options.messages.is_none() && options.tools.is_none() && options.source.is_none());
    }
}
