use std::{ops::Deref, sync::Arc};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::context_memory::{
        AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
        AgentContextMemoryServiceHandle, ContextMemoryServiceError, ContextMessage, PromptOrigin,
        vacuous_content::trim_ecmascript_whitespace,
    },
    kosong::contract::message::{ContentPart, Message, Role},
};

pub trait AgentSystemReminderServiceContract: Send + Sync {
    fn append_system_reminder(
        &self,
        content: &str,
        origin: PromptOrigin,
    ) -> Result<ContextMessage, ContextMemoryServiceError>;
}

#[derive(Clone)]
pub struct AgentSystemReminderServiceHandle(pub Arc<dyn AgentSystemReminderServiceContract>);

impl Deref for AgentSystemReminderServiceHandle {
    type Target = dyn AgentSystemReminderServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_SYSTEM_REMINDER_SERVICE_ID: ServiceIdentifier<AgentSystemReminderServiceHandle> =
    ServiceIdentifier::new("agentSystemReminderService");

pub struct AgentSystemReminderService {
    context: Arc<dyn AgentContextMemoryServiceContract>,
}

impl AgentSystemReminderService {
    pub fn new(context: Arc<dyn AgentContextMemoryServiceContract>) -> Self {
        Self { context }
    }

    // Original: systemReminderService.ts, dependency-injected constructor.
    pub fn from_handle(context: AgentContextMemoryServiceHandle) -> Self {
        Self::new(context.0)
    }
}

// Original: systemReminderService.ts, registerScopedService(..., Eager,
// "systemReminder").
pub fn register_agent_system_reminder_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_SYSTEM_REMINDER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let service: Arc<dyn AgentSystemReminderServiceContract> =
                Arc::new(AgentSystemReminderService::from_handle((*context).clone()));
            Ok(AgentSystemReminderServiceHandle(service))
        }),
        InstantiationType::Eager,
        "systemReminder",
    );
}

impl AgentSystemReminderServiceContract for AgentSystemReminderService {
    // Original:
    //   packages/agent-core-v2/src/agent/systemReminder/systemReminderService.ts
    //   AgentSystemReminderService.appendSystemReminder()
    fn append_system_reminder(
        &self,
        content: &str,
        origin: PromptOrigin,
    ) -> Result<ContextMessage, ContextMemoryServiceError> {
        let message = ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text {
                    text: format!(
                        "<system-reminder>\n{}\n</system-reminder>",
                        trim_ecmascript_whitespace(content)
                    ),
                }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: Some(origin),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        };
        self.context.append(vec![message.clone()])?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::agent::context_memory::{
        ContextCompactionInput, ContextCompactionResult, LoopRecordedEvent, UndoCut,
        compute_undo_cut,
    };

    #[derive(Default)]
    struct MemoryContext(Mutex<Vec<ContextMessage>>);

    impl AgentContextMemoryServiceContract for MemoryContext {
        fn get(&self) -> crate::agent::context_memory::ContextMemorySnapshot {
            self.0.lock().unwrap().clone().into()
        }

        fn append(&self, messages: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError> {
            self.0.lock().unwrap().extend(messages);
            Ok(())
        }

        fn append_loop_event(
            &self,
            _event: LoopRecordedEvent,
        ) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), ContextMemoryServiceError> {
            self.0.lock().unwrap().clear();
            Ok(())
        }

        fn undo(&self, count: f64) -> Result<UndoCut, ContextMemoryServiceError> {
            Ok(compute_undo_cut(&self.get(), count))
        }

        fn apply_compaction(
            &self,
            input: ContextCompactionInput,
        ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
            Ok(ContextCompactionResult {
                summary: input.summary.clone(),
                context_summary: input.context_summary.unwrap_or(input.summary),
                compacted_count: input.compacted_count,
                tokens_before: input.tokens_before,
                tokens_after: input.tokens_after.unwrap_or(0.0),
                kept_user_message_count: input.kept_user_message_count.unwrap_or(0.0),
                kept_head_user_message_count: input.kept_head_user_message_count,
                dropped_count: input.dropped_count,
            })
        }
    }

    #[test]
    fn wraps_trimmed_content_appends_and_returns_the_same_message() {
        let context = Arc::new(MemoryContext::default());
        let contract: Arc<dyn AgentContextMemoryServiceContract> = context.clone();
        let service = AgentSystemReminderService::new(contract);
        let returned = service
            .append_system_reminder(
                " \u{00a0}remember this\u{feff} ",
                PromptOrigin::Injection {
                    variant: "test".into(),
                },
            )
            .unwrap();

        let stored = context.get();
        assert_eq!(stored.len(), 1);
        assert_eq!(&stored[0], &returned);
        assert!(matches!(
            &returned.message.content[..],
            [ContentPart::Text { text }]
                if text == "<system-reminder>\nremember this\n</system-reminder>"
        ));
        assert!(matches!(
            returned.origin,
            Some(PromptOrigin::Injection { ref variant }) if variant == "test"
        ));
    }
}
