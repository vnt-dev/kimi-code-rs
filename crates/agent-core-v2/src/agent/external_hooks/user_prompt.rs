use super::types::{HookAction, HookResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedHookResult {
    pub event: String,
    pub message: String,
    pub text: String,
}

// Original:
//   packages/agent-core-v2/src/agent/externalHooks/user-prompt.ts
//   renderHookResult()
pub fn render_hook_result(event: &str, message: &str) -> String {
    format!("<hook_result hook_event=\"{event}\">\n{message}\n</hook_result>")
}

// Original: user-prompt.ts, renderUserPromptHookResult().
pub fn render_user_prompt_hook_result(
    results: Option<&[HookResult]>,
) -> Option<RenderedHookResult> {
    let messages: Vec<&str> = results
        .unwrap_or_default()
        .iter()
        .filter(|result| result.action != HookAction::Block)
        .filter_map(user_prompt_hook_message)
        .collect();
    if messages.is_empty() {
        return None;
    }
    let display_message = messages.join("\n\n");
    let text = messages
        .iter()
        .map(|message| render_hook_result("UserPromptSubmit", message))
        .collect::<Vec<_>>()
        .join("\n");
    Some(RenderedHookResult {
        event: "UserPromptSubmit".into(),
        message: display_message,
        text,
    })
}

// Original: user-prompt.ts, renderUserPromptHookBlockResult().
pub fn render_user_prompt_hook_block_result(
    results: Option<&[HookResult]>,
) -> Option<RenderedHookResult> {
    let block = results
        .unwrap_or_default()
        .iter()
        .find(|result| result.action == HookAction::Block)?;
    let message = block
        .message
        .as_deref()
        .map(trim_ecmascript)
        .filter(|message| !message.is_empty())
        .or_else(|| {
            block
                .reason
                .as_deref()
                .map(trim_ecmascript)
                .filter(|reason| !reason.is_empty())
        })
        .unwrap_or("Blocked by UserPromptSubmit hook");
    Some(RenderedHookResult {
        event: "UserPromptSubmit".into(),
        message: message.into(),
        text: render_hook_result("UserPromptSubmit", message),
    })
}

// Original: user-prompt.ts, userPromptHookMessage().
fn user_prompt_hook_message(result: &HookResult) -> Option<&str> {
    if result.timed_out == Some(true) || result.exit_code.is_some_and(|code| code != 0) {
        return None;
    }
    result
        .message
        .as_deref()
        .map(trim_ecmascript)
        .filter(|message| !message.is_empty())
        .or_else(|| {
            result
                .stdout
                .as_deref()
                .map(trim_ecmascript)
                .filter(|stdout| !stdout.is_empty())
        })
}

// JavaScript String.prototype.trim includes U+FEFF in addition to Unicode
// White_Space. Keeping it explicit avoids a serialization-visible edge-case.
fn trim_ecmascript(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(action: HookAction) -> HookResult {
        HookResult {
            action,
            message: None,
            reason: None,
            stdout: None,
            stderr: None,
            exit_code: None,
            timed_out: None,
            structured_output: None,
        }
    }

    #[test]
    fn renders_exact_hook_envelope_without_escaping() {
        assert_eq!(
            render_hook_result("Custom", "a<&b"),
            "<hook_result hook_event=\"Custom\">\na<&b\n</hook_result>"
        );
    }

    #[test]
    fn combines_success_messages_and_stdout_in_source_order() {
        let mut first = result(HookAction::Allow);
        first.message = Some("  first \n".into());
        first.stdout = Some("ignored".into());
        let mut second = result(HookAction::Allow);
        second.stdout = Some("\u{feff}second\u{feff}".into());
        let mut blocked = result(HookAction::Block);
        blocked.message = Some("not displayed".into());

        let rendered = render_user_prompt_hook_result(Some(&[first, second, blocked])).unwrap();
        assert_eq!(rendered.event, "UserPromptSubmit");
        assert_eq!(rendered.message, "first\n\nsecond");
        assert_eq!(
            rendered.text,
            "<hook_result hook_event=\"UserPromptSubmit\">\nfirst\n</hook_result>\n\
<hook_result hook_event=\"UserPromptSubmit\">\nsecond\n</hook_result>"
        );
    }

    #[test]
    fn omits_timed_out_nonzero_and_empty_allow_results() {
        let mut timed_out = result(HookAction::Allow);
        timed_out.message = Some("late".into());
        timed_out.timed_out = Some(true);
        let mut failed = result(HookAction::Allow);
        failed.stdout = Some("failure output".into());
        failed.exit_code = Some(2);
        let mut empty = result(HookAction::Allow);
        empty.message = Some(" \n ".into());
        empty.stdout = Some("\t".into());
        assert_eq!(
            render_user_prompt_hook_result(Some(&[timed_out, failed, empty])),
            None
        );
        assert_eq!(render_user_prompt_hook_result(None), None);
    }

    #[test]
    fn block_uses_first_result_and_message_reason_default_precedence() {
        let mut first = result(HookAction::Block);
        first.message = Some("  visible message  ".into());
        first.reason = Some("reason".into());
        let mut second = result(HookAction::Block);
        second.message = Some("second".into());
        assert_eq!(
            render_user_prompt_hook_block_result(Some(&[first, second]))
                .unwrap()
                .message,
            "visible message"
        );

        let mut reason = result(HookAction::Block);
        reason.message = Some("".into());
        reason.reason = Some(" why ".into());
        assert_eq!(
            render_user_prompt_hook_block_result(Some(&[reason]))
                .unwrap()
                .message,
            "why"
        );

        let fallback = result(HookAction::Block);
        let rendered = render_user_prompt_hook_block_result(Some(&[fallback])).unwrap();
        assert_eq!(rendered.message, "Blocked by UserPromptSubmit hook");
        assert_eq!(
            rendered.text,
            "<hook_result hook_event=\"UserPromptSubmit\">\n\
Blocked by UserPromptSubmit hook\n</hook_result>"
        );
        assert_eq!(render_user_prompt_hook_block_result(None), None);
    }
}
