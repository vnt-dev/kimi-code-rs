//! Pure helper methods used by `AgentTaskService`.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`.

use crate::_base::utils::abort::user_cancellation_reason;
use crate::_base::utils::xml_escape::{escape_xml, escape_xml_attr};
use serde_json::Value;

use super::{AgentTaskInfo, AgentTaskOutputSnapshot};

// Original: taskService.ts, emptyOutputSnapshot().
pub fn empty_output_snapshot() -> AgentTaskOutputSnapshot {
    AgentTaskOutputSnapshot::default()
}

// Original: taskService.ts, agentTaskNotificationChildren(). Persisted output
// takes precedence over an in-memory preview even when that preview is present.
pub fn agent_task_notification_children(output: &AgentTaskOutputSnapshot) -> Option<Vec<String>> {
    if output.full_output_available
        && let Some(path) = &output.output_path
    {
        return Some(vec![render_output_file_block(
            path,
            output.output_size_bytes,
        )]);
    }
    if output.preview.is_empty() {
        None
    } else {
        Some(vec![render_output_preview_block(output)])
    }
}

// Original: taskService.ts, renderOutputFileBlock().
pub fn render_output_file_block(output_path: &str, output_size_bytes: usize) -> String {
    format!(
        "<output-file path=\"{}\" bytes=\"{output_size_bytes}\">\nRead the output file to retrieve the result: {}\n</output-file>",
        escape_xml_attr(output_path),
        escape_xml(output_path)
    )
}

// Original: taskService.ts, renderOutputPreviewBlock().
pub fn render_output_preview_block(output: &AgentTaskOutputSnapshot) -> String {
    let explanation = if output.truncated {
        format!(
            "Showing the last {} bytes. No persisted full output is available.",
            output.preview_bytes
        )
    } else {
        "No persisted full output is available; this preview is the currently buffered task output."
            .into()
    };
    format!(
        "<output-preview bytes=\"{}\" total_bytes=\"{}\" truncated=\"{}\">\n{explanation}\n{}\n</output-preview>",
        output.preview_bytes,
        output.output_size_bytes,
        output.truncated,
        escape_xml(&output.preview)
    )
}

// Original: taskService.ts, shouldListTask(). Terminal foreground tasks stay
// hidden even from the all-tasks view.
pub fn should_list_task(info: &AgentTaskInfo, active_only: bool) -> bool {
    if !info.base.status.is_terminal() {
        true
    } else if active_only {
        false
    } else {
        info.base.detached != Some(false)
    }
}

// Original: taskService.ts, newerRestoredTask(). Terminal state wins over a
// running state; otherwise later endedAt wins, with the loaded value breaking
// ties and also winning when neither value has an end time.
pub fn newer_restored_task(existing: AgentTaskInfo, loaded: AgentTaskInfo) -> AgentTaskInfo {
    let existing_terminal = existing.base.status.is_terminal();
    let loaded_terminal = loaded.base.status.is_terminal();
    if existing_terminal && !loaded_terminal {
        return existing;
    }
    if !existing_terminal && loaded_terminal {
        return loaded;
    }
    match (existing.base.ended_at, loaded.base.ended_at) {
        (Some(existing_end), Some(loaded_end)) => {
            if loaded_end >= existing_end {
                loaded
            } else {
                existing
            }
        }
        (Some(_), None) => existing,
        (None, Some(_)) | (None, None) => loaded,
    }
}

// Original: taskService.ts, buildAgentTaskNotificationBody(). Agent tasks that
// did not complete include the exact recovery instructions used by the
// TypeScript service when their agent id differs from the task id.
pub fn build_agent_task_notification_body(info: &AgentTaskInfo) -> String {
    let base_line = if info.base.status == super::AgentTaskStatus::TimedOut {
        format!("{} timed out.", info.base.description)
    } else if info.base.status == super::AgentTaskStatus::Killed
        && is_serialized_user_cancellation(info.base.stop_reason.as_deref())
    {
        format!("{} was stopped by user.", info.base.description)
    } else if let Some(reason) = &info.base.stop_reason {
        let status = if info.base.status == super::AgentTaskStatus::Killed {
            "was stopped"
        } else {
            agent_task_status_text(info.base.status)
        };
        format!("{} {status}. Reason: {reason}", info.base.description)
    } else {
        format!(
            "{} {}.",
            info.base.description,
            agent_task_status_text(info.base.status)
        )
    };

    if info.kind != "agent" || info.base.status == super::AgentTaskStatus::Completed {
        return base_line;
    }
    let Some(agent_id) = info
        .details
        .get("agentId")
        .and_then(serde_json::Value::as_str)
    else {
        return base_line;
    };
    if agent_id == info.base.task_id {
        return base_line;
    }

    format!(
        "{base_line}\n\nTo recover or continue this subagent, call Agent(resume=\"{agent_id}\", prompt=\"Pick up where you left off; redo the last tool call if its result was never observed.\").\nUse agent_id (\"{agent_id}\"), NOT source_id / task_id (\"{}\") — the two look alike but only agent_id is accepted by the resume parameter.\nAdd run_in_background=true to keep it backgrounded, or omit it to take the result inline in the current turn.\nThe subagent retains its full prior context across the restart, but any in-flight tool call lost its result and may need to be redone.",
        info.base.task_id
    )
}

// Original: taskService.ts, normalizeReason().
pub fn normalize_reason(reason: Option<&str>) -> Option<&str> {
    reason.map(str::trim).filter(|reason| !reason.is_empty())
}

// Original: taskService.ts, isSerializedUserCancellation(). This compares the
// persisted text because the original Error object is not serialized.
pub fn is_serialized_user_cancellation(reason: Option<&str>) -> bool {
    reason.is_some_and(|reason| reason == user_cancellation_reason().to_string())
}

fn agent_task_status_text(status: super::AgentTaskStatus) -> &'static str {
    match status {
        super::AgentTaskStatus::Running => "running",
        super::AgentTaskStatus::Completed => "completed",
        super::AgentTaskStatus::Failed => "failed",
        super::AgentTaskStatus::TimedOut => "timed_out",
        super::AgentTaskStatus::Killed => "killed",
        super::AgentTaskStatus::Lost => "lost",
    }
}

// Original: taskService.ts, TaskNotificationOrigin. The status intentionally
// remains a string: isTaskOrigin() accepted any string rather than validating
// it against AgentTaskStatus, including while replaying historical records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskNotificationOrigin {
    pub task_id: String,
    pub status: String,
    pub notification_id: String,
}

// Original: taskService.ts, isTaskOrigin(). Both the retired
// `background_task` discriminator and the current `task` discriminator are
// accepted for replay compatibility.
pub fn task_notification_origin(value: &Value) -> Option<TaskNotificationOrigin> {
    let value = value.as_object()?;
    if !matches!(
        value.get("kind").and_then(Value::as_str),
        Some("background_task" | "task")
    ) {
        return None;
    }
    Some(TaskNotificationOrigin {
        task_id: value.get("taskId")?.as_str()?.into(),
        status: value.get("status")?.as_str()?.into(),
        notification_id: value.get("notificationId")?.as_str()?.into(),
    })
}

// Original: taskService.ts, notificationKey(). NUL separators preserve the
// original collision-resistant key format used by delivery de-duplication.
pub fn notification_key(origin: &TaskNotificationOrigin) -> String {
    format!(
        "{}\0{}\0{}",
        origin.task_id, origin.status, origin.notification_id
    )
}

// Original: taskService.ts, taskOriginFromMessage().
pub fn task_origin_from_message(message: &Value) -> Option<TaskNotificationOrigin> {
    task_notification_origin(message.as_object()?.get("origin")?)
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::agent::task::{AgentTaskInfoBase, AgentTaskStatus};

    fn task(
        status: AgentTaskStatus,
        detached: Option<bool>,
        ended_at: Option<i64>,
        marker: &str,
    ) -> AgentTaskInfo {
        AgentTaskInfo {
            base: AgentTaskInfoBase {
                task_id: "bash-12345678".into(),
                description: marker.into(),
                status,
                detached,
                started_at: 1,
                ended_at,
                stop_reason: None,
                terminal_notification_suppressed: None,
                timeout_ms: None,
            },
            kind: "process".into(),
            details: Map::new(),
        }
    }

    #[test]
    fn notification_children_prefer_file_then_preview_and_escape_xml() {
        assert_eq!(
            agent_task_notification_children(&empty_output_snapshot()),
            None
        );
        let preview = AgentTaskOutputSnapshot {
            output_size_bytes: 8,
            preview_bytes: 4,
            truncated: true,
            preview: "<&>".into(),
            ..AgentTaskOutputSnapshot::default()
        };
        assert_eq!(
            agent_task_notification_children(&preview).unwrap()[0],
            "<output-preview bytes=\"4\" total_bytes=\"8\" truncated=\"true\">\nShowing the last 4 bytes. No persisted full output is available.\n&lt;&amp;&gt;\n</output-preview>"
        );
        let persisted = AgentTaskOutputSnapshot {
            output_path: Some("/tmp/a&\"b".into()),
            output_size_bytes: 12,
            full_output_available: true,
            preview: "ignored".into(),
            ..AgentTaskOutputSnapshot::default()
        };
        assert_eq!(
            agent_task_notification_children(&persisted).unwrap()[0],
            "<output-file path=\"/tmp/a&amp;&quot;b\" bytes=\"12\">\nRead the output file to retrieve the result: /tmp/a&amp;&quot;b\n</output-file>"
        );
    }

    #[test]
    fn listing_keeps_active_tasks_and_only_detached_terminal_tasks() {
        assert!(should_list_task(
            &task(AgentTaskStatus::Running, Some(false), None, "active"),
            true
        ));
        assert!(!should_list_task(
            &task(AgentTaskStatus::Completed, Some(true), Some(2), "done"),
            true
        ));
        assert!(should_list_task(
            &task(AgentTaskStatus::Completed, Some(true), Some(2), "done"),
            false
        ));
        assert!(!should_list_task(
            &task(AgentTaskStatus::Completed, Some(false), Some(2), "done"),
            false
        ));
    }

    #[test]
    fn restored_conflicts_follow_terminal_and_end_time_precedence() {
        let running = task(AgentTaskStatus::Running, Some(true), None, "running");
        let terminal = task(AgentTaskStatus::Failed, Some(true), Some(5), "terminal");
        assert_eq!(
            newer_restored_task(terminal.clone(), running.clone())
                .base
                .description,
            "terminal"
        );
        assert_eq!(
            newer_restored_task(running, terminal).base.description,
            "terminal"
        );
        let old = task(AgentTaskStatus::Completed, Some(true), Some(5), "old");
        let tie = task(AgentTaskStatus::Failed, Some(true), Some(5), "loaded");
        assert_eq!(newer_restored_task(old, tie).base.description, "loaded");
        let existing = task(AgentTaskStatus::Running, Some(true), None, "existing");
        let loaded = task(AgentTaskStatus::Running, Some(true), None, "loaded");
        assert_eq!(
            newer_restored_task(existing, loaded).base.description,
            "loaded"
        );
    }

    #[test]
    fn notification_body_preserves_terminal_reason_wording() {
        let mut info = task(AgentTaskStatus::TimedOut, Some(true), Some(2), "Build");
        info.base.stop_reason = Some("ignored timeout detail".into());
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Build timed out."
        );

        info.base.status = AgentTaskStatus::Killed;
        info.base.stop_reason = Some(user_cancellation_reason().to_string());
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Build was stopped by user."
        );

        info.base.stop_reason = Some("Session closed".into());
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Build was stopped. Reason: Session closed"
        );

        info.base.status = AgentTaskStatus::Failed;
        info.base.stop_reason = Some("exit 2".into());
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Build failed. Reason: exit 2"
        );
    }

    #[test]
    fn failed_agent_notification_includes_recovery_only_for_distinct_agent_id() {
        let mut info = task(AgentTaskStatus::Lost, Some(true), Some(2), "Explore agent");
        info.kind = "agent".into();
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Explore agent lost."
        );

        info.details.insert(
            "agentId".into(),
            serde_json::Value::String("agent-42".into()),
        );
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Explore agent lost.\n\nTo recover or continue this subagent, call Agent(resume=\"agent-42\", prompt=\"Pick up where you left off; redo the last tool call if its result was never observed.\").\nUse agent_id (\"agent-42\"), NOT source_id / task_id (\"bash-12345678\") — the two look alike but only agent_id is accepted by the resume parameter.\nAdd run_in_background=true to keep it backgrounded, or omit it to take the result inline in the current turn.\nThe subagent retains its full prior context across the restart, but any in-flight tool call lost its result and may need to be redone."
        );

        info.base.status = AgentTaskStatus::Completed;
        assert_eq!(
            build_agent_task_notification_body(&info),
            "Explore agent completed."
        );
    }

    #[test]
    fn reason_helpers_match_serialized_typescript_behavior() {
        assert_eq!(normalize_reason(None), None);
        assert_eq!(normalize_reason(Some(" \n\t")), None);
        assert_eq!(normalize_reason(Some("  stopped  ")), Some("stopped"));
        assert!(is_serialized_user_cancellation(Some("Aborted by the user")));
        assert!(!is_serialized_user_cancellation(Some(
            " Aborted by the user "
        )));
        assert!(!is_serialized_user_cancellation(None));
    }

    #[test]
    fn task_origins_accept_current_and_legacy_discriminators() {
        for kind in ["task", "background_task"] {
            let origin = serde_json::json!({
                "kind": kind,
                "taskId": "agent-1",
                "status": "future_status",
                "notificationId": "notice-1",
                "ignored": true
            });
            let parsed = task_notification_origin(&origin).unwrap();
            assert_eq!(parsed.task_id, "agent-1");
            assert_eq!(parsed.status, "future_status");
            assert_eq!(
                notification_key(&parsed),
                "agent-1\0future_status\0notice-1"
            );
            assert_eq!(
                task_origin_from_message(&serde_json::json!({ "origin": origin })),
                Some(parsed)
            );
        }
    }

    #[test]
    fn task_origin_guards_reject_partial_or_wrongly_typed_values() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({
                "kind": "cron_job",
                "taskId": "t",
                "status": "completed",
                "notificationId": "n"
            }),
            serde_json::json!({
                "kind": "task",
                "taskId": 1,
                "status": "completed",
                "notificationId": "n"
            }),
            serde_json::json!({
                "kind": "task",
                "taskId": "t",
                "status": null,
                "notificationId": "n"
            }),
        ] {
            assert_eq!(task_notification_origin(&value), None);
        }
        assert_eq!(task_origin_from_message(&serde_json::json!({})), None);
        assert_eq!(task_origin_from_message(&serde_json::json!([])), None);
    }
}
