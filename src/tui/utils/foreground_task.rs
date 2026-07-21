use std::cmp::Ordering;

use crate::sdk::types::{BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus};

fn is_detachable_foreground_task(task: &BackgroundTaskInfo) -> bool {
    task.detached == Some(false)
        && task.status == BackgroundTaskStatus::Running
        && matches!(
            task.kind,
            BackgroundTaskKind::Process { .. } | BackgroundTaskKind::Agent { .. }
        )
}

// Original:
//   apps/kimi-code/src/tui/utils/foreground-task.ts
//   pickForegroundTasks()
pub fn pick_foreground_tasks(tasks: &[BackgroundTaskInfo]) -> Vec<BackgroundTaskInfo> {
    let mut foreground: Vec<_> = tasks
        .iter()
        .filter(|task| is_detachable_foreground_task(task))
        .cloned()
        .collect();
    foreground.sort_by(|left, right| {
        right
            .started_at
            .partial_cmp(&left.started_at)
            .unwrap_or(Ordering::Equal)
    });
    foreground
}

// Original:
//   apps/kimi-code/src/tui/utils/foreground-task.ts
//   pickForegroundTask()
pub fn pick_foreground_task(tasks: &[BackgroundTaskInfo]) -> Option<BackgroundTaskInfo> {
    pick_foreground_tasks(tasks).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::{pick_foreground_task, pick_foreground_tasks};
    use crate::sdk::types::{BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus};

    fn task(task_id: &str, started_at: f64) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            task_id: task_id.to_owned(),
            description: "Bash: sleep 10".to_owned(),
            status: BackgroundTaskStatus::Running,
            detached: Some(false),
            started_at,
            ended_at: None,
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
            kind: BackgroundTaskKind::Process {
                command: "sleep 10".to_owned(),
                pid: 1_234,
                exit_code: None,
            },
        }
    }

    #[test]
    fn returns_none_for_empty_detached_or_terminal_tasks() {
        assert_eq!(pick_foreground_task(&[]), None);

        let mut detached = task("bash-detached", 1_000.0);
        detached.detached = Some(true);
        assert_eq!(pick_foreground_task(&[detached]), None);

        for status in [
            BackgroundTaskStatus::Completed,
            BackgroundTaskStatus::Failed,
            BackgroundTaskStatus::TimedOut,
            BackgroundTaskStatus::Killed,
            BackgroundTaskStatus::Lost,
        ] {
            let mut terminal = task("bash-terminal", 1_000.0);
            terminal.status = status;
            assert_eq!(pick_foreground_task(&[terminal]), None);
        }
    }

    #[test]
    fn omitted_legacy_detached_field_is_not_foreground() {
        let mut legacy = task("bash-legacy", 1_000.0);
        legacy.detached = None;
        assert_eq!(pick_foreground_task(&[legacy]), None);
    }

    #[test]
    fn excludes_question_tasks() {
        let mut question = task("question-1", 1_000.0);
        question.kind = BackgroundTaskKind::Question {
            question_count: 1,
            tool_call_id: None,
        };
        assert_eq!(pick_foreground_task(&[question]), None);
    }

    #[test]
    fn returns_the_most_recent_foreground_task() {
        let older = task("bash-old", 1_000.0);
        let newer = task("bash-new", 2_000.0);
        assert_eq!(
            pick_foreground_task(&[older, newer])
                .expect("newer task")
                .task_id,
            "bash-new"
        );
    }

    #[test]
    fn ignores_a_newer_detached_task() {
        let foreground = task("bash-fg", 1_000.0);
        let mut background = task("bash-bg", 9_999.0);
        background.detached = Some(true);
        assert_eq!(
            pick_foreground_task(&[background, foreground])
                .expect("foreground task")
                .task_id,
            "bash-fg"
        );
    }

    #[test]
    fn accepts_agent_foreground_tasks() {
        let mut agent = task("agent-aaaaaaaa", 1_000.0);
        agent.kind = BackgroundTaskKind::Agent {
            agent_id: Some("child-1".to_owned()),
            subagent_type: Some("coder".to_owned()),
        };
        assert_eq!(
            pick_foreground_task(&[agent]).expect("agent task").task_id,
            "agent-aaaaaaaa"
        );
    }

    #[test]
    fn returns_all_matches_most_recent_first() {
        let first = task("bash-a", 1_000.0);
        let mut latest = task("agent-b", 3_000.0);
        latest.kind = BackgroundTaskKind::Agent {
            agent_id: None,
            subagent_type: None,
        };
        let middle = task("bash-c", 2_000.0);
        assert_eq!(
            pick_foreground_tasks(&[first, latest, middle])
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            ["agent-b", "bash-c", "bash-a"]
        );
    }

    #[test]
    fn nan_start_times_keep_stable_input_order() {
        let first = task("first", f64::NAN);
        let second = task("second", 2_000.0);
        assert_eq!(
            pick_foreground_tasks(&[first, second])
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn preserves_the_original_tagged_json_shape() {
        let task = task("bash-a", 1_000.0);
        let value = serde_json::to_value(&task).expect("serialize task");
        assert_eq!(value["taskId"], "bash-a");
        assert_eq!(value["kind"], "process");
        assert_eq!(value["status"], "running");
        assert_eq!(value["detached"], false);
        assert!(value["exitCode"].is_null());
        assert!(value.get("stopReason").is_none());
        assert_eq!(
            serde_json::from_value::<BackgroundTaskInfo>(value).expect("deserialize task"),
            task
        );
    }
}
