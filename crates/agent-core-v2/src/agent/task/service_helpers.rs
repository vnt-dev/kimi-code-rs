//! Pure helper methods used by `AgentTaskService`.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`.

use crate::_base::utils::abort::user_cancellation_reason;
use crate::_base::utils::xml_escape::{escape_xml, escape_xml_attr};
use crate::agent::context_memory::{ContextMessage, PromptOrigin};
use serde_json::Value;

use super::{
    AgentTaskInfo, AgentTaskInfoBase, AgentTaskOutputSnapshot, AgentTaskSettlement,
    AgentTaskSettlementStatus, AgentTaskStatus,
};

pub const MAX_RETAINED_OUTPUT_BYTES: usize = 1024 * 1024;
pub const MAX_TASK_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const TASK_ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
pub const ACTIVE_BACKGROUND_TASK_INJECTION_VARIANT: &str = "background_task_status";
const ACTIVE_BACKGROUND_TASK_GUIDANCE: &str = "The conversation was compacted, so the earlier messages that started these background tasks are gone — but the tasks are still running from before. Do not start duplicates. Use TaskOutput to fetch a task’s result, TaskList to list them, and TaskStop to cancel one.";

// Original: taskService.ts, generateTaskId(). The OS random source is the
// Rust counterpart of node:crypto randomBytes(); byte-wise modulo mapping and
// the eight-character suffix are unchanged.
pub fn generate_task_id(kind: &str) -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)?;
    Ok(task_id_from_bytes(kind, bytes))
}

// Original: taskService.ts, activeBackgroundTaskReminder(). The pending bit is
// consumed before checking the active list, including when the list is empty.
pub fn active_background_task_reminder(
    pending: &mut bool,
    active_tasks: &[AgentTaskInfo],
) -> Option<String> {
    if !*pending {
        return None;
    }
    *pending = false;
    if active_tasks.is_empty() {
        return None;
    }
    Some(format!(
        "{ACTIVE_BACKGROUND_TASK_GUIDANCE}\n\n{}",
        super::tools::format_task_list(active_tasks, true)
    ))
}

fn task_id_from_bytes(kind: &str, bytes: [u8; 8]) -> String {
    let mut task_id = String::with_capacity(kind.len() + 9);
    task_id.push_str(kind);
    task_id.push('-');
    for byte in bytes {
        task_id.push(TASK_ID_ALPHABET[usize::from(byte) % TASK_ID_ALPHABET.len()] as char);
    }
    task_id
}

// Original: taskService.ts, outputLimitReason().
pub fn output_limit_reason() -> String {
    let mib = MAX_TASK_OUTPUT_BYTES / (1024 * 1024);
    format!(
        "Output limit exceeded: the command produced more than {mib} MiB and was terminated. Redirect large output to a file (e.g. `command > out.txt`) and inspect it in slices instead."
    )
}

// Original: taskService.ts, coerceTimeoutSettlement(). Once the manager's
// timeout path wins, a subsequently reported killed settlement remains a
// timeout while retaining the original stop reason.
pub fn coerce_timeout_settlement(
    timed_out: bool,
    mut settlement: AgentTaskSettlement,
) -> AgentTaskSettlement {
    if timed_out && settlement.status == AgentTaskSettlementStatus::Killed {
        settlement.status = AgentTaskSettlementStatus::TimedOut;
    }
    settlement
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Too many background tasks are already running.")]
pub struct TooManyBackgroundTasksError;

// Original: taskService.ts, startsDetached().
pub fn starts_detached(detached: Option<bool>) -> bool {
    detached != Some(false)
}

// Original: taskService.ts, activeTaskCount().
pub fn active_task_count<'a>(
    tasks: impl IntoIterator<Item = (&'a AgentTaskStatus, Option<bool>)>,
) -> usize {
    tasks
        .into_iter()
        .filter(|(status, detached)| !status.is_terminal() && starts_detached(*detached))
        .count()
}

// Original: taskService.ts, assertCanRegister(). Foreground registrations do
// not consume or enforce the configured background-task quota.
pub fn check_task_registration(
    detached: bool,
    active_detached_tasks: usize,
    max_running_tasks: Option<u64>,
) -> Result<(), TooManyBackgroundTasksError> {
    let Some(max_running_tasks) = max_running_tasks else {
        return Ok(());
    };
    if !detached || (active_detached_tasks as u128) < u128::from(max_running_tasks) {
        Ok(())
    } else {
        Err(TooManyBackgroundTasksError)
    }
}

// Original: taskService.ts, markLoadedTasksLost() per-entry state transition.
// Persistence remains the caller's async responsibility so entries can be
// written sequentially in the same order as the source loop.
pub fn mark_loaded_task_lost(mut info: AgentTaskInfo, now_ms: i64) -> Option<AgentTaskInfo> {
    if info.base.status.is_terminal() {
        return None;
    }
    info.base.status = AgentTaskStatus::Lost;
    if info.base.ended_at.is_none() {
        info.base.ended_at = Some(now_ms);
    }
    Some(info)
}

// Original: taskService.ts, settleTask() state mutation. Cleanup, persistence,
// notifications, foreground release, and waiter resolution remain ordered
// side effects for the service layer after this atomic state commit succeeds.
pub fn apply_task_settlement(
    base: &mut AgentTaskInfoBase,
    settlement: AgentTaskSettlement,
    now_ms: i64,
) -> bool {
    if base.status.is_terminal() {
        return false;
    }
    let status = match settlement.status {
        AgentTaskSettlementStatus::Completed => AgentTaskStatus::Completed,
        AgentTaskSettlementStatus::Failed => AgentTaskStatus::Failed,
        AgentTaskSettlementStatus::TimedOut => AgentTaskStatus::TimedOut,
        AgentTaskSettlementStatus::Killed => AgentTaskStatus::Killed,
    };
    base.status = status;
    base.ended_at = Some(now_ms);
    base.stop_reason = match settlement.stop_reason {
        Some(reason) => Some(reason),
        None if status == AgentTaskStatus::Killed => base.stop_reason.take(),
        None => None,
    };
    true
}

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

pub(crate) fn agent_task_status_text(status: super::AgentTaskStatus) -> &'static str {
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

// Original: taskService.ts, isCompactionSplice(). A compaction reminder is
// armed only when history was actually deleted and the replacement includes a
// compaction summary message.
pub fn is_compaction_splice(delete_count: usize, messages: &[ContextMessage]) -> bool {
    delete_count > 0
        && messages.iter().any(|message| {
            matches!(
                message.origin.as_ref(),
                Some(PromptOrigin::CompactionSummary)
            )
        })
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;
    use crate::agent::task::{AgentTaskInfoBase, AgentTaskStatus};
    use crate::kosong::contract::message::{Message, Role};

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

    #[test]
    fn compaction_splice_requires_deletion_and_a_summary_replacement() {
        let message = |origin| ContextMessage {
            message: Message::new(Role::User, vec![], vec![]),
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
        };
        let summary = message(Some(PromptOrigin::CompactionSummary));
        let ordinary = message(Some(PromptOrigin::User));

        assert!(is_compaction_splice(
            1,
            &[ordinary.clone(), summary.clone()]
        ));
        assert!(!is_compaction_splice(0, &[summary]));
        assert!(!is_compaction_splice(2, &[ordinary]));
        assert!(!is_compaction_splice(2, &[]));
    }

    #[test]
    fn output_limit_reason_preserves_limit_and_guidance() {
        assert_eq!(MAX_RETAINED_OUTPUT_BYTES, 1024 * 1024);
        assert_eq!(MAX_TASK_OUTPUT_BYTES, 16 * 1024 * 1024);
        assert_eq!(
            output_limit_reason(),
            "Output limit exceeded: the command produced more than 16 MiB and was terminated. Redirect large output to a file (e.g. `command > out.txt`) and inspect it in slices instead."
        );
    }

    #[test]
    fn timeout_only_coerces_killed_settlements() {
        let settlement = |status| AgentTaskSettlement {
            status,
            stop_reason: Some("manager stopped it".into()),
        };
        assert_eq!(
            coerce_timeout_settlement(true, settlement(AgentTaskSettlementStatus::Killed)),
            settlement(AgentTaskSettlementStatus::TimedOut)
        );
        assert_eq!(
            coerce_timeout_settlement(false, settlement(AgentTaskSettlementStatus::Killed)),
            settlement(AgentTaskSettlementStatus::Killed)
        );
        assert_eq!(
            coerce_timeout_settlement(true, settlement(AgentTaskSettlementStatus::Failed)),
            settlement(AgentTaskSettlementStatus::Failed)
        );
    }

    #[test]
    fn task_id_generation_preserves_byte_modulo_mapping() {
        assert_eq!(
            task_id_from_bytes("agent", [0, 9, 10, 35, 36, 37, 254, 255]),
            "agent-09az0123"
        );

        let generated = generate_task_id("bash").unwrap();
        assert_eq!(generated.len(), 13);
        assert!(generated.starts_with("bash-"));
        assert!(
            generated[5..]
                .bytes()
                .all(|byte| TASK_ID_ALPHABET.contains(&byte))
        );
    }

    #[test]
    fn active_task_reminder_consumes_pending_state_once() {
        assert_eq!(
            ACTIVE_BACKGROUND_TASK_INJECTION_VARIANT,
            "background_task_status"
        );
        let active = task(AgentTaskStatus::Running, Some(true), None, "Build index");
        let mut pending = false;
        assert_eq!(
            active_background_task_reminder(&mut pending, std::slice::from_ref(&active)),
            None
        );

        pending = true;
        let reminder =
            active_background_task_reminder(&mut pending, std::slice::from_ref(&active)).unwrap();
        assert!(!pending);
        assert_eq!(
            reminder,
            "The conversation was compacted, so the earlier messages that started these background tasks are gone — but the tasks are still running from before. Do not start duplicates. Use TaskOutput to fetch a task’s result, TaskList to list them, and TaskStop to cancel one.\n\nactive_background_tasks: 1\ntask_id: bash-12345678\ndescription: Build index\nstatus: running\ndetached: true\nstarted_at: 1\nkind: process"
        );
        assert_eq!(active_background_task_reminder(&mut pending, &[]), None);

        pending = true;
        assert_eq!(active_background_task_reminder(&mut pending, &[]), None);
        assert!(!pending);
    }

    #[test]
    fn registration_quota_counts_only_active_initially_detached_tasks() {
        let running = AgentTaskStatus::Running;
        let completed = AgentTaskStatus::Completed;
        let failed = AgentTaskStatus::Failed;
        assert!(starts_detached(None));
        assert!(starts_detached(Some(true)));
        assert!(!starts_detached(Some(false)));
        assert_eq!(
            active_task_count([
                (&running, None),
                (&running, Some(true)),
                (&running, Some(false)),
                (&completed, Some(true)),
                (&failed, None),
            ]),
            2
        );

        assert_eq!(check_task_registration(true, 99, None), Ok(()));
        assert_eq!(check_task_registration(false, 2, Some(2)), Ok(()));
        assert_eq!(check_task_registration(true, 1, Some(2)), Ok(()));
        assert_eq!(
            check_task_registration(true, 2, Some(2)),
            Err(TooManyBackgroundTasksError)
        );
        assert_eq!(
            TooManyBackgroundTasksError.to_string(),
            "Too many background tasks are already running."
        );
    }

    #[test]
    fn restored_active_tasks_become_lost_without_overwriting_end_time() {
        let running = task(AgentTaskStatus::Running, Some(true), None, "running");
        let lost = mark_loaded_task_lost(running, 50).unwrap();
        assert_eq!(lost.base.status, AgentTaskStatus::Lost);
        assert_eq!(lost.base.ended_at, Some(50));

        let ended = task(AgentTaskStatus::Running, Some(true), Some(30), "ended");
        let lost = mark_loaded_task_lost(ended, 50).unwrap();
        assert_eq!(lost.base.ended_at, Some(30));

        for status in [
            AgentTaskStatus::Completed,
            AgentTaskStatus::Failed,
            AgentTaskStatus::TimedOut,
            AgentTaskStatus::Killed,
            AgentTaskStatus::Lost,
        ] {
            assert_eq!(
                mark_loaded_task_lost(task(status, Some(true), Some(20), "terminal"), 50),
                None
            );
        }
    }

    #[test]
    fn settlement_is_single_commit_and_preserves_reason_only_for_killed() {
        let mut info = task(AgentTaskStatus::Running, Some(true), None, "running");
        info.base.stop_reason = Some("requested stop".into());
        assert!(apply_task_settlement(
            &mut info.base,
            AgentTaskSettlement {
                status: AgentTaskSettlementStatus::Killed,
                stop_reason: None,
            },
            80,
        ));
        assert_eq!(info.base.status, AgentTaskStatus::Killed);
        assert_eq!(info.base.ended_at, Some(80));
        assert_eq!(info.base.stop_reason.as_deref(), Some("requested stop"));
        assert!(!apply_task_settlement(
            &mut info.base,
            AgentTaskSettlement {
                status: AgentTaskSettlementStatus::Failed,
                stop_reason: Some("late".into()),
            },
            90,
        ));
        assert_eq!(info.base.status, AgentTaskStatus::Killed);
        assert_eq!(info.base.ended_at, Some(80));

        for status in [
            AgentTaskSettlementStatus::Completed,
            AgentTaskSettlementStatus::Failed,
            AgentTaskSettlementStatus::TimedOut,
        ] {
            let mut info = task(AgentTaskStatus::Running, Some(true), None, "running");
            info.base.stop_reason = Some("stale".into());
            assert!(apply_task_settlement(
                &mut info.base,
                AgentTaskSettlement {
                    status,
                    stop_reason: None,
                },
                100,
            ));
            assert_eq!(info.base.stop_reason, None);
        }

        let mut info = task(AgentTaskStatus::Running, Some(true), None, "running");
        assert!(apply_task_settlement(
            &mut info.base,
            AgentTaskSettlement {
                status: AgentTaskSettlementStatus::Failed,
                stop_reason: Some("exit 2".into()),
            },
            110,
        ));
        assert_eq!(info.base.stop_reason.as_deref(), Some("exit 2"));
    }
}
