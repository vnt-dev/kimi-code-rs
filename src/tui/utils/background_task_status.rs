use crate::{
    sdk::types::{BackgroundTaskInfo, BackgroundTaskKind, BackgroundTaskStatus},
    tui::types::{BackgroundAgentStatusData, BackgroundAgentStatusPhase},
};

const MAX_DETAIL_LENGTH: usize = 240;

fn truncate(value: Option<&str>) -> Option<String> {
    let collapsed = value?.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }

    if collapsed.chars().count() <= MAX_DETAIL_LENGTH {
        return Some(collapsed);
    }

    let prefix = collapsed
        .chars()
        .take(MAX_DETAIL_LENGTH - 3)
        .collect::<String>();
    Some(format!("{prefix}..."))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTaskTranscriptPhase {
    Started,
    Updated,
    Terminal,
}

fn phase_from_status(status: BackgroundTaskStatus) -> BackgroundAgentStatusPhase {
    match status {
        BackgroundTaskStatus::Running => BackgroundAgentStatusPhase::Started,
        BackgroundTaskStatus::Completed => BackgroundAgentStatusPhase::Completed,
        BackgroundTaskStatus::Failed
        | BackgroundTaskStatus::TimedOut
        | BackgroundTaskStatus::Killed
        | BackgroundTaskStatus::Lost => BackgroundAgentStatusPhase::Failed,
    }
}

fn subject_for(info: &BackgroundTaskInfo) -> &'static str {
    match info.kind {
        BackgroundTaskKind::Agent { .. } => "agent task",
        BackgroundTaskKind::Question { .. } => "question task",
        BackgroundTaskKind::Process { .. } => "bash task",
    }
}

fn headline_for(info: &BackgroundTaskInfo) -> String {
    let subject = subject_for(info);
    match info.status {
        BackgroundTaskStatus::Running => format!("{subject} started in background"),
        BackgroundTaskStatus::Completed => format!("{subject} completed in background"),
        BackgroundTaskStatus::Failed => format!("{subject} failed in background"),
        BackgroundTaskStatus::TimedOut => format!("{subject} timed out"),
        BackgroundTaskStatus::Killed => format!("{subject} stopped"),
        BackgroundTaskStatus::Lost => format!("{subject} lost"),
    }
}

fn detail_for(info: &BackgroundTaskInfo) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(description) = truncate(Some(&info.description)) {
        parts.push(description);
    }

    if matches!(
        info.status,
        BackgroundTaskStatus::Completed | BackgroundTaskStatus::Failed
    ) && let BackgroundTaskKind::Process {
        exit_code: Some(exit_code),
        ..
    } = info.kind
    {
        parts.push(format!("exit {exit_code}"));
    }
    if info.status == BackgroundTaskStatus::Killed {
        let reason = truncate(info.stop_reason.as_deref());
        parts.push(
            reason
                .map(|reason| format!("stopped — {reason}"))
                .unwrap_or_else(|| "stopped".to_owned()),
        );
    }
    if info.status == BackgroundTaskStatus::Failed
        && let Some(reason) = truncate(info.stop_reason.as_deref())
    {
        parts.push(reason);
    }
    if info.status == BackgroundTaskStatus::TimedOut {
        parts.push("timed out".to_owned());
    }
    if info.status == BackgroundTaskStatus::Lost {
        parts.push("session restarted before completion".to_owned());
    }

    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Original:
///   apps/kimi-code/src/tui/utils/background-task-status.ts
///   formatBackgroundTaskTranscript()
pub fn format_background_task_transcript(info: &BackgroundTaskInfo) -> BackgroundAgentStatusData {
    BackgroundAgentStatusData {
        phase: phase_from_status(info.status),
        headline: headline_for(info),
        detail: detail_for(info),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_task(status: BackgroundTaskStatus) -> BackgroundTaskInfo {
        BackgroundTaskInfo {
            task_id: "bash-abcd1234".to_owned(),
            description: "dev server".to_owned(),
            status,
            detached: None,
            started_at: 1_000.0,
            ended_at: None,
            stop_reason: None,
            terminal_notification_suppressed: None,
            timeout_ms: None,
            kind: BackgroundTaskKind::Process {
                command: "npm run dev".to_owned(),
                pid: 1234,
                exit_code: None,
            },
        }
    }

    #[test]
    fn formats_started_task_kinds() {
        let bash = format_background_task_transcript(&process_task(BackgroundTaskStatus::Running));
        let mut agent = process_task(BackgroundTaskStatus::Running);
        agent.kind = BackgroundTaskKind::Agent {
            agent_id: Some("agent-child".to_owned()),
            subagent_type: Some("coder".to_owned()),
        };
        let mut question = process_task(BackgroundTaskStatus::Running);
        question.kind = BackgroundTaskKind::Question {
            question_count: 1,
            tool_call_id: None,
        };

        assert_eq!(bash.headline, "bash task started in background");
        assert_eq!(
            format_background_task_transcript(&agent).headline,
            "agent task started in background"
        );
        assert_eq!(
            format_background_task_transcript(&question).headline,
            "question task started in background"
        );
    }

    #[test]
    fn includes_process_exit_code_for_completed_and_failed_tasks() {
        for (status, code, phase) in [
            (
                BackgroundTaskStatus::Completed,
                0,
                BackgroundAgentStatusPhase::Completed,
            ),
            (
                BackgroundTaskStatus::Failed,
                2,
                BackgroundAgentStatusPhase::Failed,
            ),
        ] {
            let mut task = process_task(status);
            task.kind = BackgroundTaskKind::Process {
                command: "npm run dev".to_owned(),
                pid: 1234,
                exit_code: Some(code),
            };

            let data = format_background_task_transcript(&task);
            assert_eq!(data.phase, phase);
            assert!(
                data.detail
                    .as_deref()
                    .is_some_and(|value| value.contains(&format!("exit {code}")))
            );
        }
    }

    #[test]
    fn maps_terminal_status_details() {
        let mut killed = process_task(BackgroundTaskStatus::Killed);
        killed.stop_reason = Some(" user   request ".to_owned());
        let timed_out = process_task(BackgroundTaskStatus::TimedOut);
        let lost = process_task(BackgroundTaskStatus::Lost);

        assert_eq!(
            format_background_task_transcript(&killed).detail.as_deref(),
            Some("dev server · stopped — user request")
        );
        assert_eq!(
            format_background_task_transcript(&timed_out)
                .detail
                .as_deref(),
            Some("dev server · timed out")
        );
        assert_eq!(
            format_background_task_transcript(&lost).detail.as_deref(),
            Some("dev server · session restarted before completion")
        );
    }

    #[test]
    fn handles_every_background_task_status() {
        for status in [
            BackgroundTaskStatus::Running,
            BackgroundTaskStatus::Completed,
            BackgroundTaskStatus::Failed,
            BackgroundTaskStatus::TimedOut,
            BackgroundTaskStatus::Killed,
            BackgroundTaskStatus::Lost,
        ] {
            let data = format_background_task_transcript(&process_task(status));
            assert!(!data.headline.is_empty());
        }
    }
}
