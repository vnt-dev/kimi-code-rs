//! Pure output formatting from `session/subagent/tools/agent.ts`.

use super::subagent_task::SubagentHandle;

pub const USER_INTERRUPTED_SUBAGENT_MESSAGE: &str =
    "The subagent was stopped before it finished by user.";
pub const SUBAGENT_STOPPED_MESSAGE: &str = "The subagent was stopped before it finished.";

pub fn format_background_agent_result(
    task_id: &str,
    handle: &SubagentHandle,
    description: &str,
    allow_background: bool,
) -> String {
    let next = if allow_background {
        "next_step: The completion arrives automatically in a later turn — do NOT wait, poll, or call TaskOutput on it; continue with other work or hand back to the user. (If you have nothing to do until it finishes, run such tasks in the foreground next time.)"
    } else {
        "next_step: The completion arrives automatically in a later turn."
    };
    format!(
        "task_id: {task_id}\nstatus: running\nagent_id: {}\nactual_subagent_type: {}\nautomatic_notification: true\n\ndescription: {description}\n\n{next}\nresume_hint: To continue or recover this same subagent later, call Agent(resume=\"{}\", prompt=\"...\"). The parameter is agent_id (\"{}\"), NOT task_id (\"{task_id}\") or source_id from a later <notification>. Recovery cases: a later <notification type=\"task.lost\" | \"task.failed\" | \"task.killed\"> for this subagent — its conversation history is preserved across session restarts and resume will pick it up.",
        handle.agent_id, handle.profile_name, handle.agent_id, handle.agent_id
    )
}

pub fn format_foreground_agent_success(handle: &SubagentHandle, result: &str) -> String {
    format!(
        "agent_id: {}\nactual_subagent_type: {}\nstatus: completed\n\n[summary]\n{result}",
        handle.agent_id, handle.profile_name
    )
}
pub fn format_foreground_agent_failure(
    handle: &SubagentHandle,
    message: &str,
    timed_out: bool,
) -> String {
    let mut text = format!(
        "agent_id: {}\nactual_subagent_type: {}\nstatus: failed\n\nsubagent error: {message}",
        handle.agent_id, handle.profile_name
    );
    if timed_out {
        text.push_str(&format!("\nresume_hint: Continue with Agent(resume=\"{}\", prompt=\"continue\"). Use agent_id only; do not set subagent_type. The subagent retains its prior context; redo any unfinished tool call if its result was lost.", handle.agent_id));
    }
    text
}
pub fn format_subagent_stopped_message(reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|value| !value.is_empty()) {
        Some("Aborted by the user") => USER_INTERRUPTED_SUBAGENT_MESSAGE.into(),
        Some(reason) => format!("{SUBAGENT_STOPPED_MESSAGE} Reason: {reason}"),
        None => SUBAGENT_STOPPED_MESSAGE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopped_message_preserves_source_branches() {
        assert_eq!(
            format_subagent_stopped_message(None),
            SUBAGENT_STOPPED_MESSAGE
        );
        assert_eq!(
            format_subagent_stopped_message(Some(" Aborted by the user ")),
            USER_INTERRUPTED_SUBAGENT_MESSAGE
        );
        assert_eq!(
            format_subagent_stopped_message(Some("lost")),
            "The subagent was stopped before it finished. Reason: lost"
        );
    }
}
