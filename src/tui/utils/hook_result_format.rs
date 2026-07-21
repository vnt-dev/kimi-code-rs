use crate::sdk::types::HookResultEvent;

/// Original:
///   apps/kimi-code/src/tui/utils/hook-result-format.ts
///   formatHookResultMarkdown()
pub fn format_hook_result_markdown(event: &HookResultEvent) -> String {
    format!(
        "*{}*\n\n{}",
        format_hook_result_title(event),
        format_hook_result_body(event)
    )
}

/// Original:
///   apps/kimi-code/src/tui/utils/hook-result-format.ts
///   formatHookResultPlain()
pub fn format_hook_result_plain(event: &HookResultEvent) -> String {
    format!(
        "{}\n\n{}",
        format_hook_result_title(event),
        format_hook_result_body(event)
    )
}

fn format_hook_result_title(event: &HookResultEvent) -> String {
    let blocked = if event.blocked { " blocked" } else { "" };
    format!("{} hook{blocked}", event.hook_event)
}

fn format_hook_result_body(event: &HookResultEvent) -> &str {
    let content = event.content.trim();
    if content.is_empty() {
        "(empty)"
    } else {
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(content: &str, blocked: bool) -> HookResultEvent {
        HookResultEvent {
            hook_event: "before_prompt".to_owned(),
            content: content.to_owned(),
            blocked,
        }
    }

    #[test]
    fn formats_markdown_and_trims_content() {
        assert_eq!(
            format_hook_result_markdown(&event("  explanation\n", false)),
            "*before_prompt hook*\n\nexplanation"
        );
    }

    #[test]
    fn formats_plain_blocked_empty_result() {
        assert_eq!(
            format_hook_result_plain(&event(" \n\t", true)),
            "before_prompt hook blocked\n\n(empty)"
        );
    }
}
