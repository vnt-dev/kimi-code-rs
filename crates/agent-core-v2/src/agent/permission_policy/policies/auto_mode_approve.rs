//! Auto-permission-mode approval policy.
//!
//! Original: `agent/permissionPolicy/policies/auto-mode-approve.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_mode::AgentPermissionModeServiceContract,
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
};

pub struct AutoModeApprovePermissionPolicy {
    mode: Arc<dyn AgentPermissionModeServiceContract>,
}

impl AutoModeApprovePermissionPolicy {
    pub fn new(mode: Arc<dyn AgentPermissionModeServiceContract>) -> Self {
        Self { mode }
    }
}

fn auto_mode_result(
    mode: crate::agent::permission_policy::PermissionMode,
) -> Option<PermissionPolicyResult> {
    (mode == crate::agent::permission_policy::PermissionMode::Auto).then_some(
        PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        },
    )
}

impl PermissionPolicy for AutoModeApprovePermissionPolicy {
    fn name(&self) -> &str {
        "auto-mode-approve"
    }

    fn evaluate<'a>(
        &'a self,
        _context: &'a crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { auto_mode_result(self.mode.mode()) }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::permission_policy::PermissionMode;

    #[test]
    fn only_auto_mode_approves() {
        assert!(matches!(
            auto_mode_result(PermissionMode::Auto),
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(auto_mode_result(PermissionMode::Manual).is_none());
        assert!(auto_mode_result(PermissionMode::Yolo).is_none());
    }
}
