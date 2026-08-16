//! Prompt-created loop step requests.
//!
//! Original: `packages/agent-core-v2/src/agent/prompt/promptStepRequests.ts`.

use std::sync::Arc;

use crate::{
    agent::{
        context_memory::{ContextMessage, PromptOrigin},
        loop_::{
            LiveUserMessage, StepRequest, StepRequestAdmission, StepRequestCore,
            StepRequestOptions, TurnSeed,
        },
        media::gate_image_format_parts,
        system_reminder::AgentSystemReminderServiceContract,
    },
    kosong::contract::message::ContentPart,
};

type RecordSteer = Arc<dyn Fn(ContextMessage) + Send + Sync>;
type ForgetSteer = Arc<dyn Fn() + Send + Sync>;

struct UserMessageStepRequest {
    core: StepRequestCore,
    kind: &'static str,
    message: ContextMessage,
    captions: Vec<String>,
    reminders: Arc<dyn AgentSystemReminderServiceContract>,
    user_message: Option<LiveUserMessage>,
}

impl UserMessageStepRequest {
    fn new(
        kind: &'static str,
        message: ContextMessage,
        captions: Vec<String>,
        reminders: Arc<dyn AgentSystemReminderServiceContract>,
        user_message: Option<LiveUserMessage>,
        options: StepRequestOptions,
    ) -> Self {
        let mut message = message;
        message.message.content = Arc::new(gate_image_format_parts(&message.message.content));
        Self {
            core: StepRequestCore::new(options),
            kind,
            message,
            captions,
            reminders,
            user_message,
        }
    }
    fn seed(&self) -> TurnSeed {
        TurnSeed {
            input: self.message.message.content.as_ref().clone(),
            origin: self.message.origin.clone().unwrap_or(PromptOrigin::User),
            user_message: self.user_message.clone(),
        }
    }
    fn before_materialize(&self) {
        for caption in &self.captions {
            let _ = self.reminders.append_system_reminder(
                caption,
                PromptOrigin::Injection {
                    variant: "image_compression".into(),
                },
            );
        }
    }
    fn messages(&self) -> Vec<ContextMessage> {
        if self.message.message.content.is_empty() {
            Vec::new()
        } else {
            vec![self.message.clone()]
        }
    }
}

pub struct PromptStepRequest(UserMessageStepRequest);
impl PromptStepRequest {
    pub fn new(
        message: ContextMessage,
        captions: Vec<String>,
        reminders: Arc<dyn AgentSystemReminderServiceContract>,
        user_message: LiveUserMessage,
    ) -> Self {
        Self(UserMessageStepRequest::new(
            "prompt",
            message,
            captions,
            reminders,
            Some(user_message),
            StepRequestOptions {
                admission: Some(StepRequestAdmission::NewTurn),
                ..Default::default()
            },
        ))
    }
}
impl StepRequest for PromptStepRequest {
    fn core(&self) -> &StepRequestCore {
        &self.0.core
    }
    fn kind(&self) -> &str {
        self.0.kind
    }
    fn turn_seed(&self) -> Option<TurnSeed> {
        Some(self.0.seed())
    }
    fn on_will_materialize(&self) {
        self.0.before_materialize()
    }
    fn resolve_context_messages(&self) -> Vec<ContextMessage> {
        self.0.messages()
    }
}

pub struct SteerStepRequest {
    request: UserMessageStepRequest,
    record_steer: RecordSteer,
    forget_steer: ForgetSteer,
}
impl SteerStepRequest {
    pub fn new(
        message: ContextMessage,
        captions: Vec<String>,
        reminders: Arc<dyn AgentSystemReminderServiceContract>,
        record_steer: RecordSteer,
        forget_steer: ForgetSteer,
        admission: Option<StepRequestAdmission>,
    ) -> Self {
        Self {
            request: UserMessageStepRequest::new(
                "steer",
                message,
                captions,
                reminders,
                None,
                StepRequestOptions {
                    mergeable: Some(true),
                    turn_scoped: Some(false),
                    admission: Some(admission.unwrap_or(StepRequestAdmission::ActiveTurnOnly)),
                },
            ),
            record_steer,
            forget_steer,
        }
    }
}
impl StepRequest for SteerStepRequest {
    fn core(&self) -> &StepRequestCore {
        &self.request.core
    }
    fn kind(&self) -> &str {
        self.request.kind
    }
    fn turn_seed(&self) -> Option<TurnSeed> {
        Some(self.request.seed())
    }
    fn on_will_materialize(&self) {
        (self.record_steer)(self.request.message.clone());
        self.request.before_materialize()
    }
    fn resolve_context_messages(&self) -> Vec<ContextMessage> {
        self.request.messages()
    }
    fn on_settled(&self) {
        (self.forget_steer)()
    }
}

pub struct RetryStepRequest {
    core: StepRequestCore,
}
impl Default for RetryStepRequest {
    fn default() -> Self {
        Self::new()
    }
}
impl RetryStepRequest {
    pub fn new() -> Self {
        Self {
            core: StepRequestCore::new(StepRequestOptions {
                admission: Some(StepRequestAdmission::NewTurn),
                ..Default::default()
            }),
        }
    }
}
impl StepRequest for RetryStepRequest {
    fn core(&self) -> &StepRequestCore {
        &self.core
    }
    fn kind(&self) -> &str {
        "retry"
    }
    fn turn_seed(&self) -> Option<TurnSeed> {
        Some(TurnSeed {
            input: Vec::<ContentPart>::new(),
            origin: PromptOrigin::Retry { trigger: None },
            user_message: None,
        })
    }
    fn resolve_context_messages(&self) -> Vec<ContextMessage> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::context_memory::PromptOrigin,
        kosong::contract::message::{Message, Role},
    };
    struct NoopReminder;
    impl AgentSystemReminderServiceContract for NoopReminder {
        fn append_system_reminder(
            &self,
            _: &str,
            _: PromptOrigin,
        ) -> Result<ContextMessage, crate::agent::context_memory::ContextMemoryServiceError>
        {
            Ok(ContextMessage {
                message: Message::new(Role::User, Vec::new(), Vec::new()),
                id: None,
                provider_message_id: None,
                origin: None,
                is_error: None,
                note: None,
                attachments: Vec::new(),
            })
        }
    }
    fn message(content: Vec<ContentPart>) -> ContextMessage {
        ContextMessage {
            message: Message::new(Role::User, content, Vec::new()),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::User),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }
    #[test]
    fn prompt_steer_and_retry_preserve_admission_and_lazy_message_rules() {
        let reminders: Arc<dyn AgentSystemReminderServiceContract> = Arc::new(NoopReminder);
        let prompt = PromptStepRequest::new(
            message(vec![ContentPart::Text {
                text: "hello".into(),
            }]),
            Vec::new(),
            reminders.clone(),
            LiveUserMessage {
                prompt_id: "prompt-1".into(),
                user_message_id: "message-1".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                content: Vec::new(),
                origin: PromptOrigin::User,
            },
        );
        assert_eq!(prompt.admission(), StepRequestAdmission::NewTurn);
        assert_eq!(prompt.kind(), "prompt");
        assert_eq!(prompt.resolve_context_messages().len(), 1);
        let steer = SteerStepRequest::new(
            message(Vec::new()),
            Vec::new(),
            reminders,
            Arc::new(|_| {}),
            Arc::new(|| {}),
            None,
        );
        assert!(steer.mergeable());
        assert!(!steer.turn_scoped());
        assert_eq!(steer.admission(), StepRequestAdmission::ActiveTurnOnly);
        assert!(steer.resolve_context_messages().is_empty());
        let retry = RetryStepRequest::new();
        assert_eq!(retry.admission(), StepRequestAdmission::NewTurn);
        assert!(matches!(
            retry.turn_seed().unwrap().origin,
            PromptOrigin::Retry { trigger: None }
        ));
    }
}
