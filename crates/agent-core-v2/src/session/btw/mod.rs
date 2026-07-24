//! Side-question (`btw`) agent constants.
//!
//! Original: `packages/agent-core-v2/src/session/btw/btw.ts`.

pub const TOOL_CALL_DISABLED_MESSAGE: &str =
    "Tool calls are disabled for side questions. Answer with text only.";

pub const SIDE_QUESTION_SYSTEM_REMINDER: &str = "This is a side-channel conversation with the user. You should answer user questions directly based on what you already know.

IMPORTANT:
- You are a separate, lightweight instance.
- The main agent continues independently; do not reference being interrupted.
- Do not call any tools. All tool calls are disabled and will be rejected.
  Even though tool definitions are visible in this request, they exist only
  for technical reasons (prompt cache). You must not use them.
- Respond only with text based on what you already know from the conversation
  and this side-channel conversation.
- Follow-up turns may happen in this side-channel conversation.
- If you do not know the answer, say so directly.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_question_prompt_and_tool_rejection_are_verbatim() {
        assert_eq!(
            TOOL_CALL_DISABLED_MESSAGE,
            "Tool calls are disabled for side questions. Answer with text only."
        );
        assert!(SIDE_QUESTION_SYSTEM_REMINDER.starts_with("This is a side-channel"));
        assert!(SIDE_QUESTION_SYSTEM_REMINDER.contains("You must not use them."));
        assert!(!SIDE_QUESTION_SYSTEM_REMINDER.ends_with('\n'));
    }
}
