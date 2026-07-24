//! Auto mode guard against asking the user for a decision.
//!
//! Original: `agent/permissionPolicy/policies/auto-mode-ask-user-question-deny.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_mode::AgentPermissionModeServiceContract,
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
};

pub const AUTO_MODE_ASK_USER_QUESTION_DENIED_MESSAGE: &str = "AskUserQuestion is disabled while auto permission mode is active. Make a reasonable decision and continue without asking the user.";

pub struct AutoModeAskUserQuestionDenyPermissionPolicy {
    mode: Arc<dyn AgentPermissionModeServiceContract>,
}

impl AutoModeAskUserQuestionDenyPermissionPolicy {
    pub fn new(mode: Arc<dyn AgentPermissionModeServiceContract>) -> Self {
        Self { mode }
    }
}

fn auto_mode_ask_user_question_result(
    mode: crate::agent::permission_policy::PermissionMode,
    tool_name: &str,
) -> Option<PermissionPolicyResult> {
    (mode == crate::agent::permission_policy::PermissionMode::Auto
        && tool_name == "AskUserQuestion")
        .then_some(PermissionPolicyResult::Deny {
            reason: None,
            message: Some(AUTO_MODE_ASK_USER_QUESTION_DENIED_MESSAGE.into()),
        })
}

impl PermissionPolicy for AutoModeAskUserQuestionDenyPermissionPolicy {
    fn name(&self) -> &str {
        "auto-mode-ask-user-question-deny"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { auto_mode_ask_user_question_result(self.mode.mode(), &context.tool_call.name) }
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::permission_policy::PermissionMode;

    #[test]
    fn only_auto_mode_ask_user_question_is_denied() {
        assert!(matches!(
            auto_mode_ask_user_question_result(PermissionMode::Auto, "AskUserQuestion"),
            Some(PermissionPolicyResult::Deny { ref message, .. }) if message.as_deref() == Some(AUTO_MODE_ASK_USER_QUESTION_DENIED_MESSAGE)
        ));
        assert!(auto_mode_ask_user_question_result(PermissionMode::Auto, "Read").is_none());
        assert!(
            auto_mode_ask_user_question_result(PermissionMode::Manual, "AskUserQuestion").is_none()
        );
    }
}
