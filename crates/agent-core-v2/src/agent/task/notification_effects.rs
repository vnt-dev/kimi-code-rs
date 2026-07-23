//! Delivery effects for built terminal task notifications.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `notifyAgentTask()`, `restoreAgentTaskNotification()`, and
//! `fireNotificationHook()`.

use std::sync::Arc;

use crate::{
    agent::{
        context_memory::{
            AgentContextMemoryServiceContract, ContextMemoryServiceError, ContextMessage,
        },
        loop_::{AgentLoopServiceContract, LoopValue, StepRequest},
    },
    app::event::event_bus::EventBusHandle,
    kosong::contract::message::{Message, Role},
};

use super::{
    AgentTaskNotificationBuildContext, task_notification_domain_event,
    task_notification_step_request,
};

#[derive(Debug, thiserror::Error)]
pub enum AgentTaskNotificationEffectError {
    #[error(transparent)]
    Loop(#[from] LoopValue),
    #[error(transparent)]
    Context(#[from] ContextMemoryServiceError),
}

#[derive(Clone)]
pub struct AgentTaskNotificationEffects {
    context: Arc<dyn AgentContextMemoryServiceContract>,
    event_bus: EventBusHandle,
    loop_service: Arc<dyn AgentLoopServiceContract>,
}

impl AgentTaskNotificationEffects {
    pub fn new(
        context: Arc<dyn AgentContextMemoryServiceContract>,
        event_bus: EventBusHandle,
        loop_service: Arc<dyn AgentLoopServiceContract>,
    ) -> Self {
        Self {
            context,
            event_bus,
            loop_service,
        }
    }

    // Original: AgentTaskService.notifyAgentTask() after notification build.
    pub fn enqueue(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> Result<(), AgentTaskNotificationEffectError> {
        let message = notification_message(built);
        let request: Arc<dyn StepRequest> = Arc::new(task_notification_step_request(message));
        self.loop_service.enqueue(request, None)?;
        self.publish_hook(built);
        Ok(())
    }

    // Original: AgentTaskService.restoreAgentTaskNotification() after build.
    pub fn restore(
        &self,
        built: &AgentTaskNotificationBuildContext,
    ) -> Result<(), AgentTaskNotificationEffectError> {
        self.context.append(vec![notification_message(built)])?;
        self.publish_hook(built);
        Ok(())
    }

    fn publish_hook(&self, built: &AgentTaskNotificationBuildContext) {
        self.event_bus
            .publish(task_notification_domain_event(&built.hook_context));
    }
}

fn notification_message(built: &AgentTaskNotificationBuildContext) -> ContextMessage {
    ContextMessage {
        message: Message::new(Role::User, built.content.clone(), vec![]),
        id: None,
        provider_message_id: None,
        origin: Some(built.origin.clone()),
        is_error: None,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value};

    use super::*;
    use crate::{
        agent::{
            context_memory::PromptOrigin,
            task::{AgentTaskNotificationContext, AgentTaskNotificationSeverity},
        },
        kosong::contract::message::ContentPart,
    };

    fn built() -> AgentTaskNotificationBuildContext {
        AgentTaskNotificationBuildContext {
            content: vec![ContentPart::Text {
                text: "<notification>done</notification>".into(),
            }],
            origin: PromptOrigin::Task {
                task_id: "bash-12345678".into(),
                status: crate::agent::task::AgentTaskStatus::Completed,
                notification_id: "task:bash-12345678:completed".into(),
            },
            notification: Map::from_iter([("type".into(), Value::String("task.completed".into()))]),
            hook_context: AgentTaskNotificationContext {
                notification_type: "task.completed".into(),
                title: "Background process completed".into(),
                body: "done".into(),
                severity: AgentTaskNotificationSeverity::Info,
                source_kind: "background_task".into(),
                source_id: "bash-12345678".into(),
            },
        }
    }

    #[test]
    fn notification_message_preserves_user_role_content_and_task_origin() {
        let built = built();
        let message = notification_message(&built);
        assert_eq!(message.message.role, Role::User);
        assert_eq!(message.message.content, built.content);
        assert!(message.message.tool_calls.is_empty());
        assert_eq!(message.origin, Some(built.origin));
        assert_eq!(message.id, None);
        assert_eq!(message.provider_message_id, None);
    }
}
