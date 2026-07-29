//! Lazy context-message requests consumed by the agent loop step queue.
//!
//! Original: `packages/agent-core-v2/src/agent/loop/stepRequest.ts`.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::{
    agent::context_memory::{ContextMessage, PromptOrigin},
    kosong::contract::message::ContentPart,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StepRequestState {
    Pending = 0,
    Materialized = 1,
    Aborted = 2,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StepRequestAdmission {
    NewTurn,
    ActiveOrNewTurn,
    #[default]
    ActiveOrNextTurn,
    ActiveTurnOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnSeed {
    pub input: Vec<ContentPart>,
    pub origin: PromptOrigin,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StepRequestOptions {
    pub mergeable: Option<bool>,
    pub turn_scoped: Option<bool>,
    pub admission: Option<StepRequestAdmission>,
}

pub struct StepRequestCore {
    id: String,
    mergeable: bool,
    turn_scoped: bool,
    admission: StepRequestAdmission,
    state: AtomicU8,
}

impl StepRequestCore {
    pub fn new(options: StepRequestOptions) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            mergeable: options.mergeable.unwrap_or(false),
            turn_scoped: options.turn_scoped.unwrap_or(true),
            admission: options.admission.unwrap_or_default(),
            state: AtomicU8::new(StepRequestState::Pending as u8),
        }
    }

    fn state(&self) -> StepRequestState {
        match self.state.load(Ordering::Acquire) {
            0 => StepRequestState::Pending,
            1 => StepRequestState::Materialized,
            2 => StepRequestState::Aborted,
            _ => unreachable!("StepRequestCore only stores valid states"),
        }
    }

    fn transition(&self, state: StepRequestState) -> bool {
        self.state
            .compare_exchange(
                StepRequestState::Pending as u8,
                state as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

pub trait StepRequest: Send + Sync {
    fn core(&self) -> &StepRequestCore;
    fn kind(&self) -> &str;

    fn id(&self) -> &str {
        &self.core().id
    }

    fn mergeable(&self) -> bool {
        self.core().mergeable
    }

    fn turn_scoped(&self) -> bool {
        self.core().turn_scoped
    }

    fn admission(&self) -> StepRequestAdmission {
        self.core().admission
    }

    fn turn_seed(&self) -> Option<TurnSeed> {
        None
    }

    fn state(&self) -> StepRequestState {
        self.core().state()
    }

    fn aborted(&self) -> bool {
        self.state() == StepRequestState::Aborted
    }

    // Original: StepRequest.abort().
    fn abort(&self) -> bool {
        if !self.core().transition(StepRequestState::Aborted) {
            return false;
        }
        self.on_settled();
        true
    }

    fn on_will_materialize(&self) {}

    fn resolve_context_messages(&self) -> Vec<ContextMessage>;

    // Original: StepRequest.markMaterialized().
    fn mark_materialized(&self) {
        if self.core().transition(StepRequestState::Materialized) {
            self.on_settled();
        }
    }

    fn on_settled(&self) {}
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MessageStepRequestOptions {
    pub request: StepRequestOptions,
    pub kind: Option<String>,
}

pub struct MessageStepRequest {
    core: StepRequestCore,
    kind: String,
    message: ContextMessage,
}

impl MessageStepRequest {
    pub fn new(message: ContextMessage, options: MessageStepRequestOptions) -> Self {
        Self {
            core: StepRequestCore::new(options.request),
            kind: options.kind.unwrap_or_else(|| "message".into()),
            message,
        }
    }
}

impl StepRequest for MessageStepRequest {
    fn core(&self) -> &StepRequestCore {
        &self.core
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    // Original: MessageStepRequest.turnSeed.
    fn turn_seed(&self) -> Option<TurnSeed> {
        Some(TurnSeed {
            input: self.message.message.content.clone(),
            origin: self.message.origin.clone().unwrap_or(PromptOrigin::User),
        })
    }

    // Original: MessageStepRequest.resolveContextMessages().
    fn resolve_context_messages(&self) -> Vec<ContextMessage> {
        vec![self.message.clone()]
    }
}

pub struct ContinuationStepRequest {
    core: StepRequestCore,
    kind: String,
}

impl ContinuationStepRequest {
    pub fn new(options: MessageStepRequestOptions) -> Self {
        Self {
            core: StepRequestCore::new(options.request),
            kind: options.kind.unwrap_or_else(|| "continuation".into()),
        }
    }
}

impl StepRequest for ContinuationStepRequest {
    fn core(&self) -> &StepRequestCore {
        &self.core
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    // Original: ContinuationStepRequest.resolveContextMessages().
    fn resolve_context_messages(&self) -> Vec<ContextMessage> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::kosong::contract::message::{Message, Role};

    use super::*;

    fn context(origin: Option<PromptOrigin>) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: "hello".into(),
                }],
                vec![],
            ),
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn defaults_and_one_way_state_transitions_match_source() {
        let request = ContinuationStepRequest::new(MessageStepRequestOptions::default());
        assert_eq!(request.kind(), "continuation");
        assert!(!request.mergeable());
        assert!(request.turn_scoped());
        assert_eq!(request.admission(), StepRequestAdmission::ActiveOrNextTurn);
        assert_eq!(request.state(), StepRequestState::Pending);
        assert!(!request.aborted());
        assert_eq!(request.resolve_context_messages(), []);
        uuid::Uuid::parse_str(request.id()).unwrap();

        request.mark_materialized();
        assert_eq!(request.state(), StepRequestState::Materialized);
        assert!(!request.abort());
        request.mark_materialized();
        assert_eq!(request.state(), StepRequestState::Materialized);

        let aborted = ContinuationStepRequest::new(MessageStepRequestOptions::default());
        assert!(aborted.abort());
        assert!(aborted.aborted());
        assert!(!aborted.abort());
        aborted.mark_materialized();
        assert_eq!(aborted.state(), StepRequestState::Aborted);
    }

    #[test]
    fn message_request_resolves_lazily_and_supplies_turn_seed() {
        let message = context(Some(PromptOrigin::SystemTrigger {
            name: "goal".into(),
        }));
        let request = MessageStepRequest::new(
            message.clone(),
            MessageStepRequestOptions {
                request: StepRequestOptions {
                    mergeable: Some(true),
                    turn_scoped: Some(false),
                    admission: Some(StepRequestAdmission::ActiveOrNewTurn),
                },
                kind: Some("task_notification".into()),
            },
        );
        assert_eq!(request.kind(), "task_notification");
        assert!(request.mergeable());
        assert!(!request.turn_scoped());
        assert_eq!(request.admission(), StepRequestAdmission::ActiveOrNewTurn);
        assert_eq!(request.resolve_context_messages(), [message]);
        assert_eq!(
            request.turn_seed(),
            Some(TurnSeed {
                input: vec![ContentPart::Text {
                    text: "hello".into()
                }],
                origin: PromptOrigin::SystemTrigger {
                    name: "goal".into()
                },
            })
        );

        let default_origin =
            MessageStepRequest::new(context(None), MessageStepRequestOptions::default());
        assert_eq!(
            default_origin.turn_seed().unwrap().origin,
            PromptOrigin::User
        );
    }
}
