//! Two-phase terminal task notification construction.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`,
//! `buildAgentTaskNotificationContext()`.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{
    agent::{
        context_memory::{ContextMessage, PromptOrigin},
        loop_::{
            MessageStepRequest, MessageStepRequestOptions, StepRequestAdmission, StepRequestOptions,
        },
    },
    app::event::event_bus::DomainEvent,
    kosong::contract::message::ContentPart,
};

use super::{
    AgentTaskInfo, AgentTaskNotificationContext, AgentTaskNotificationSeverity,
    AgentTaskOutputSnapshot, AgentTaskStatus, TaskNotificationOrigin,
    agent_task_notification_children, agent_task_status_text, build_agent_task_notification_body,
    notification_key, render_notification_xml,
};

pub const NOTIFICATION_FALLBACK_PREVIEW_BYTES: f64 = 3_000.0;

// Original: taskService.ts, TaskNotificationStepRequest.constructor().
pub fn task_notification_step_request(message: ContextMessage) -> MessageStepRequest {
    MessageStepRequest::new(
        message,
        MessageStepRequestOptions {
            request: StepRequestOptions {
                mergeable: Some(true),
                turn_scoped: Some(false),
                admission: Some(StepRequestAdmission::ActiveOrNewTurn),
            },
            kind: Some("task_notification".into()),
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledTaskNotification {
    pub key: String,
    pub origin: PromptOrigin,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TaskNotificationDelivery {
    scheduled_keys: HashSet<String>,
    delivered_keys: HashSet<String>,
}

impl TaskNotificationDelivery {
    // Original: taskService.ts restore hook initialization from
    // TaskNotificationDeliveryModel.
    pub fn restore_delivered(&mut self, keys: impl IntoIterator<Item = String>) {
        self.delivered_keys.extend(keys);
    }

    // Original: taskService.ts, markDeliveredNotification().
    pub fn mark_delivered(&mut self, origin: &PromptOrigin) {
        if let Some(key) = prompt_task_notification_key(origin) {
            self.delivered_keys.insert(key);
        }
    }

    // Original: buildAgentTaskNotificationContext() admission, including
    // hasDeliveredNotification()'s scan of current context messages.
    pub fn try_schedule(
        &mut self,
        info: &AgentTaskInfo,
        context: &[ContextMessage],
    ) -> Option<ScheduledTaskNotification> {
        let status = agent_task_status_text(info.base.status);
        let notification_id = format!("task:{}:{status}", info.base.task_id);
        let key = notification_key(&TaskNotificationOrigin {
            task_id: info.base.task_id.clone(),
            status: status.into(),
            notification_id,
        });
        let delivered_in_context = context.iter().any(|message| {
            message
                .origin
                .as_ref()
                .and_then(prompt_task_notification_key)
                .is_some_and(|candidate| candidate == key)
        });
        try_schedule_task_notification(
            &mut self.scheduled_keys,
            &self.delivered_keys,
            info,
            delivered_in_context,
        )
    }

    pub fn is_scheduled(&self, key: &str) -> bool {
        self.scheduled_keys.contains(key)
    }

    pub fn is_delivered(&self, key: &str) -> bool {
        self.delivered_keys.contains(key)
    }
}

fn prompt_task_notification_key(origin: &PromptOrigin) -> Option<String> {
    let PromptOrigin::Task {
        task_id,
        status,
        notification_id,
    } = origin
    else {
        return None;
    };
    Some(notification_key(&TaskNotificationOrigin {
        task_id: task_id.clone(),
        status: agent_task_status_text(*status).into(),
        notification_id: notification_id.clone(),
    }))
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentTaskNotificationBuildContext {
    pub content: Vec<ContentPart>,
    pub origin: PromptOrigin,
    pub notification: Map<String, Value>,
    pub hook_context: AgentTaskNotificationContext,
}

// Original: buildAgentTaskNotificationContext() admission through insertion
// into scheduledNotificationKeys. The insertion intentionally happens before
// either output snapshot read.
pub fn try_schedule_task_notification(
    scheduled_keys: &mut HashSet<String>,
    delivered_keys: &HashSet<String>,
    info: &AgentTaskInfo,
    delivered_in_context: bool,
) -> Option<ScheduledTaskNotification> {
    if info.base.detached == Some(false) || info.base.terminal_notification_suppressed == Some(true)
    {
        return None;
    }
    let status = agent_task_status_text(info.base.status);
    let notification_id = format!("task:{}:{status}", info.base.task_id);
    let task_origin = TaskNotificationOrigin {
        task_id: info.base.task_id.clone(),
        status: status.into(),
        notification_id: notification_id.clone(),
    };
    let key = notification_key(&task_origin);
    if scheduled_keys.contains(&key) || delivered_keys.contains(&key) || delivered_in_context {
        return None;
    }
    scheduled_keys.insert(key.clone());
    Some(ScheduledTaskNotification {
        key,
        origin: PromptOrigin::Task {
            task_id: info.base.task_id.clone(),
            status: info.base.status,
            notification_id,
        },
    })
}

// Original: buildAgentTaskNotificationContext() second snapshot decision.
pub fn needs_notification_fallback_preview(output: &AgentTaskOutputSnapshot) -> bool {
    !output.full_output_available
}

// Original: buildAgentTaskNotificationContext() after the asynchronous output
// reads. A suppression observed in that window drops the notification without
// removing its scheduled key.
pub fn finish_task_notification(
    scheduled: ScheduledTaskNotification,
    info: &AgentTaskInfo,
    output: &AgentTaskOutputSnapshot,
    currently_suppressed: bool,
) -> Option<AgentTaskNotificationBuildContext> {
    if currently_suppressed {
        return None;
    }
    let status = agent_task_status_text(info.base.status);
    let notification_id = match &scheduled.origin {
        PromptOrigin::Task {
            notification_id, ..
        } => notification_id.clone(),
        _ => return None,
    };
    let mut notification = Map::from_iter([
        ("id".into(), Value::String(notification_id)),
        ("category".into(), Value::String("task".into())),
        ("type".into(), Value::String(format!("task.{status}"))),
        (
            "source_kind".into(),
            Value::String("background_task".into()),
        ),
        ("source_id".into(), Value::String(info.base.task_id.clone())),
        (
            "title".into(),
            Value::String(format!("Background {} {status}", info.kind)),
        ),
        (
            "severity".into(),
            Value::String(
                if info.base.status == AgentTaskStatus::Completed {
                    "info"
                } else {
                    "warning"
                }
                .into(),
            ),
        ),
        (
            "body".into(),
            Value::String(build_agent_task_notification_body(info)),
        ),
    ]);
    if info.kind == "agent" {
        notification.insert(
            "agent_id".into(),
            info.details.get("agentId").cloned().unwrap_or(Value::Null),
        );
    }
    if let Some(children) = agent_task_notification_children(output) {
        notification.insert(
            "children".into(),
            Value::Array(children.into_iter().map(Value::String).collect()),
        );
    }
    let xml = render_notification_xml(&notification);
    let hook_context = AgentTaskNotificationContext {
        notification_type: format!("task.{status}"),
        title: format!("Background {} {status}", info.kind),
        body: build_agent_task_notification_body(info),
        severity: if info.base.status == AgentTaskStatus::Completed {
            AgentTaskNotificationSeverity::Info
        } else {
            AgentTaskNotificationSeverity::Warning
        },
        source_kind: "background_task".into(),
        source_id: info.base.task_id.clone(),
    };
    Some(AgentTaskNotificationBuildContext {
        content: vec![ContentPart::Text { text: xml }],
        origin: scheduled.origin,
        notification,
        hook_context,
    })
}

// Original: taskService.ts, fireNotificationHook().
pub fn task_notification_domain_event(context: &AgentTaskNotificationContext) -> DomainEvent {
    DomainEvent::new(
        "task.notified",
        Map::from_iter([
            (
                "notificationType".into(),
                Value::String(context.notification_type.clone()),
            ),
            ("title".into(), Value::String(context.title.clone())),
            ("body".into(), Value::String(context.body.clone())),
            (
                "severity".into(),
                Value::String(
                    match context.severity {
                        AgentTaskNotificationSeverity::Info => "info",
                        AgentTaskNotificationSeverity::Warning => "warning",
                    }
                    .into(),
                ),
            ),
            (
                "sourceKind".into(),
                Value::String(context.source_kind.clone()),
            ),
            ("sourceId".into(), Value::String(context.source_id.clone())),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_::StepRequest;
    use crate::agent::task::AgentTaskInfoBase;
    use crate::kosong::contract::message::{Message, Role};

    fn task(status: AgentTaskStatus) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "agent-task-12345678".into(),
                description: "Explore agent".into(),
                status,
                detached: Some(true),
                started_at: 1,
                ended_at: Some(2),
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "agent".into(),
            details: Map::from_iter([("agentId".into(), Value::String("agent-42".into()))]),
        }
    }

    #[test]
    fn admission_rejects_foreground_suppressed_scheduled_and_delivered_tasks() {
        let mut scheduled = HashSet::new();
        let delivered = HashSet::new();
        let mut info = task(AgentTaskStatus::Completed);
        info.base.detached = Some(false);
        assert!(try_schedule_task_notification(&mut scheduled, &delivered, &info, false).is_none());
        info.base.detached = Some(true);
        info.base.terminal_notification_suppressed = Some(true);
        assert!(try_schedule_task_notification(&mut scheduled, &delivered, &info, false).is_none());
        info.base.terminal_notification_suppressed = None;
        assert!(try_schedule_task_notification(&mut scheduled, &delivered, &info, true).is_none());

        let first =
            try_schedule_task_notification(&mut scheduled, &delivered, &info, false).unwrap();
        assert_eq!(
            first.key,
            "agent-task-12345678\0completed\0task:agent-task-12345678:completed"
        );
        assert!(try_schedule_task_notification(&mut scheduled, &delivered, &info, false).is_none());

        let mut independently_delivered = HashSet::new();
        independently_delivered.insert(first.key);
        assert!(
            try_schedule_task_notification(
                &mut HashSet::new(),
                &independently_delivered,
                &info,
                false,
            )
            .is_none()
        );
    }

    #[test]
    fn finish_builds_exact_notification_xml_and_agent_metadata() {
        let info = task(AgentTaskStatus::Failed);
        let scheduled =
            try_schedule_task_notification(&mut HashSet::new(), &HashSet::new(), &info, false)
                .unwrap();
        let output = AgentTaskOutputSnapshot {
            output_size_bytes: 4,
            preview_bytes: 4,
            preview: "oops".into(),
            ..AgentTaskOutputSnapshot::default()
        };
        assert!(needs_notification_fallback_preview(&output));
        let context = finish_task_notification(scheduled, &info, &output, false).unwrap();
        assert_eq!(context.notification["type"], "task.failed");
        assert_eq!(context.notification["severity"], "warning");
        assert_eq!(context.notification["agent_id"], "agent-42");
        assert_eq!(
            context.hook_context,
            AgentTaskNotificationContext {
                notification_type: "task.failed".into(),
                title: "Background agent failed".into(),
                body: build_agent_task_notification_body(&info),
                severity: AgentTaskNotificationSeverity::Warning,
                source_kind: "background_task".into(),
                source_id: "agent-task-12345678".into(),
            }
        );
        assert_eq!(
            context.content,
            vec![ContentPart::Text {
                text: "<notification id=\"task:agent-task-12345678:failed\" category=\"task\" type=\"task.failed\" source_kind=\"background_task\" source_id=\"agent-task-12345678\" agent_id=\"agent-42\">\nTitle: Background agent failed\nSeverity: warning\nExplore agent failed.\n\nTo recover or continue this subagent, call Agent(resume=\"agent-42\", prompt=\"Pick up where you left off; redo the last tool call if its result was never observed.\").\nUse agent_id (\"agent-42\"), NOT source_id / task_id (\"agent-task-12345678\") — the two look alike but only agent_id is accepted by the resume parameter.\nAdd run_in_background=true to keep it backgrounded, or omit it to take the result inline in the current turn.\nThe subagent retains its full prior context across the restart, but any in-flight tool call lost its result and may need to be redone.\n<output-preview bytes=\"4\" total_bytes=\"4\" truncated=\"false\">\nNo persisted full output is available; this preview is the currently buffered task output.\noops\n</output-preview>\n</notification>".into()
            }]
        );
    }

    #[test]
    fn fallback_and_post_read_suppression_preserve_source_behavior() {
        let info = task(AgentTaskStatus::Completed);
        let scheduled =
            try_schedule_task_notification(&mut HashSet::new(), &HashSet::new(), &info, false)
                .unwrap();
        let persisted = AgentTaskOutputSnapshot {
            full_output_available: true,
            ..AgentTaskOutputSnapshot::default()
        };
        assert!(!needs_notification_fallback_preview(&persisted));
        assert!(finish_task_notification(scheduled, &info, &persisted, true).is_none());
    }

    #[test]
    fn task_notification_request_uses_active_or_new_turn_admission() {
        let message = ContextMessage {
            message: Message::new(Role::User, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::Task {
                task_id: "bash-1".into(),
                status: AgentTaskStatus::Completed,
                notification_id: "task:bash-1:completed".into(),
            }),
            is_error: None,
            note: None,
        };
        let request = task_notification_step_request(message.clone());
        assert_eq!(request.kind(), "task_notification");
        assert!(request.mergeable());
        assert!(!request.turn_scoped());
        assert_eq!(request.admission(), StepRequestAdmission::ActiveOrNewTurn);
        assert_eq!(request.resolve_context_messages(), [message]);
    }

    #[test]
    fn notification_hook_event_preserves_exact_external_field_names() {
        let event = task_notification_domain_event(&AgentTaskNotificationContext {
            notification_type: "task.completed".into(),
            title: "Background process completed".into(),
            body: "command completed.".into(),
            severity: AgentTaskNotificationSeverity::Info,
            source_kind: "background_task".into(),
            source_id: "bash-12345678".into(),
        });
        assert_eq!(
            event.into_value(),
            serde_json::json!({
                "type": "task.notified",
                "notificationType": "task.completed",
                "title": "Background process completed",
                "body": "command completed.",
                "severity": "info",
                "sourceKind": "background_task",
                "sourceId": "bash-12345678"
            })
        );
    }

    #[test]
    fn delivery_state_combines_wire_context_and_splice_deduplication() {
        let info = task(AgentTaskStatus::Completed);
        let key = "agent-task-12345678\0completed\0task:agent-task-12345678:completed";
        let origin = PromptOrigin::Task {
            task_id: "agent-task-12345678".into(),
            status: AgentTaskStatus::Completed,
            notification_id: "task:agent-task-12345678:completed".into(),
        };
        let message = ContextMessage {
            message: Message::new(Role::User, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin: Some(origin.clone()),
            is_error: None,
            note: None,
        };

        let mut from_wire = TaskNotificationDelivery::default();
        from_wire.restore_delivered([key.into()]);
        assert!(from_wire.is_delivered(key));
        assert!(from_wire.try_schedule(&info, &[]).is_none());

        let mut from_context = TaskNotificationDelivery::default();
        assert!(from_context.try_schedule(&info, &[message]).is_none());

        let mut from_splice = TaskNotificationDelivery::default();
        from_splice.mark_delivered(&origin);
        assert!(from_splice.is_delivered(key));
        assert!(from_splice.try_schedule(&info, &[]).is_none());

        let mut scheduled = TaskNotificationDelivery::default();
        let reservation = scheduled.try_schedule(&info, &[]).unwrap();
        assert_eq!(reservation.key, key);
        assert!(scheduled.is_scheduled(key));
        assert!(scheduled.try_schedule(&info, &[]).is_none());
    }
}
