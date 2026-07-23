//! Pure helper methods used by `AgentTaskService`.
//!
//! Original: `packages/agent-core-v2/src/agent/task/taskService.ts`.

use crate::_base::utils::xml_escape::{escape_xml, escape_xml_attr};

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
}
