//! Agent prompt scheduler data and service contract.
//!
//! Original: `packages/agent-core-v2/src/agent/prompt/prompt.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::{BoxFuture, Shared};
use serde::{Deserialize, Serialize};

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    agent::{
        context_memory::ContextMessage,
        loop_::{LiveUserMessage, LoopRunResult, TurnHandle},
    },
    app::event::event_bus::DomainEventPayload,
    hooks::OrderedHookSlot,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptSubmittedStatus {
    Queued,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSubmittedEvent {
    #[serde(flatten)]
    pub user_message: LiveUserMessage,
    pub status: PromptSubmittedStatus,
}

impl DomainEventPayload for PromptSubmittedEvent {
    const TYPE: &'static str = "prompt.submitted";
}

pub type PromptServiceError = Box<dyn Error + Send + Sync>;
pub type PromptServiceResult<T> = Result<T, PromptServiceError>;

#[derive(Clone, Debug)]
pub struct PromptSubmitContext {
    pub prompt_message: ContextMessage,
    pub is_steer: bool,
    pub block: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PromptInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub message: ContextMessage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptState {
    Pending,
    Running,
    Steered,
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptCompletionState {
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Clone, Debug)]
pub struct PromptCompletion {
    pub prompt_id: String,
    pub result: Option<LoopRunResult>,
    pub state: PromptCompletionState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSnapshot {
    pub id: String,
    pub user_message_id: String,
    pub created_at: String,
    pub state: PromptState,
    pub message: ContextMessage,
}

pub type PromptLaunchedFuture = Shared<BoxFuture<'static, Option<TurnHandle>>>;
pub type PromptCompletionFuture = Shared<BoxFuture<'static, PromptCompletion>>;

pub trait PromptHandleContract: Send + Sync {
    fn snapshot(&self) -> PromptSnapshot;
    fn launched(&self) -> PromptLaunchedFuture;
    fn completion(&self) -> PromptCompletionFuture;
}

#[derive(Clone)]
pub struct PromptHandle(pub Arc<dyn PromptHandleContract>);

impl Deref for PromptHandle {
    type Target = dyn PromptHandleContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptQueueSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<PromptSnapshot>,
    #[serde(default)]
    pub pending: Vec<PromptSnapshot>,
}

pub struct AgentPromptHooks {
    pub on_before_submit_prompt: OrderedHookSlot<PromptSubmitContext>,
}

impl Default for AgentPromptHooks {
    fn default() -> Self {
        Self {
            on_before_submit_prompt: OrderedHookSlot::new(),
        }
    }
}

#[async_trait]
pub trait AgentPromptServiceContract: Disposable + Send + Sync {
    async fn enqueue(&self, input: PromptInput) -> PromptServiceResult<PromptHandle>;
    fn list(&self) -> PromptQueueSnapshot;
    async fn steer(&self, prompt_ids: &[String]) -> PromptServiceResult<Vec<PromptHandle>>;
    fn abort(&self, prompt_id: &str, reason: Option<Arc<dyn Error + Send + Sync>>) -> bool;
    async fn inject(&self, message: ContextMessage) -> PromptServiceResult<Option<TurnHandle>>;
    async fn retry(&self) -> PromptServiceResult<Option<TurnHandle>>;
    fn undo(&self, count: f64) -> PromptServiceResult<usize>;
    fn clear(&self) -> PromptServiceResult<()>;
    async fn shutdown(&self) {}
    fn hooks(&self) -> &AgentPromptHooks;
}

#[derive(Clone)]
pub struct AgentPromptServiceHandle(pub Arc<dyn AgentPromptServiceContract>);

impl Deref for AgentPromptServiceHandle {
    type Target = dyn AgentPromptServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentPromptServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_PROMPT_SERVICE_ID: ServiceIdentifier<AgentPromptServiceHandle> =
    ServiceIdentifier::new("agentPromptService");

#[cfg(test)]
mod tests {
    use crate::{
        agent::context_memory::{ImageSource, MessageContent, PromptOrigin, USER_PROMPT_ORIGIN},
        kosong::contract::message::{ContentPart, Message, Role},
    };

    use super::*;

    fn message() -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: "hello".into(),
                }],
                Vec::new(),
            ),
            id: Some("message-1".into()),
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn prompt_contract_uses_original_service_identity_and_wire_names() {
        assert_eq!(AGENT_PROMPT_SERVICE_ID.to_string(), "agentPromptService");
        assert_eq!(
            serde_json::to_value(PromptSnapshot {
                id: "prompt-1".into(),
                user_message_id: "message-1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                state: PromptState::Pending,
                message: message(),
            })
            .unwrap(),
            serde_json::json!({
                "id": "prompt-1",
                "userMessageId": "message-1",
                "createdAt": "2026-01-01T00:00:00.000Z",
                "state": "pending",
                "message": {
                    "role": "user",
                    "content": [{ "type": "text", "text": "hello" }],
                    "toolCalls": [],
                    "id": "message-1",
                    "origin": USER_PROMPT_ORIGIN,
                },
            })
        );
    }

    #[test]
    fn completion_states_are_limited_to_terminal_prompt_states() {
        assert_eq!(
            serde_json::to_value(PromptCompletionState::Blocked).unwrap(),
            "blocked"
        );
        assert_eq!(
            serde_json::to_value(PromptState::Steered).unwrap(),
            "steered"
        );
    }

    #[test]
    fn submitted_event_flattens_structured_user_message_fields() {
        let event = PromptSubmittedEvent {
            user_message: LiveUserMessage {
                prompt_id: "prompt-1".into(),
                user_message_id: "message-1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                content: vec![
                    MessageContent::Text {
                        text: "inspect these".into(),
                    },
                    MessageContent::Image {
                        source: ImageSource::Url {
                            url: "image.png".into(),
                        },
                    },
                    MessageContent::Video {
                        source: ImageSource::Url {
                            url: "video.mp4".into(),
                        },
                    },
                    MessageContent::File {
                        file_id: "file-1".into(),
                        name: "notes.txt".into(),
                        media_type: "text/plain".into(),
                        size: 12,
                    },
                ],
            },
            status: PromptSubmittedStatus::Queued,
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "promptId": "prompt-1",
                "userMessageId": "message-1",
                "createdAt": "2026-01-01T00:00:00.000Z",
                "content": [
                    { "type": "text", "text": "inspect these" },
                    { "type": "image", "source": { "kind": "url", "url": "image.png" } },
                    { "type": "video", "source": { "kind": "url", "url": "video.mp4" } },
                    {
                        "type": "file",
                        "file_id": "file-1",
                        "name": "notes.txt",
                        "media_type": "text/plain",
                        "size": 12
                    }
                ],
                "status": "queued"
            })
        );
    }
}
