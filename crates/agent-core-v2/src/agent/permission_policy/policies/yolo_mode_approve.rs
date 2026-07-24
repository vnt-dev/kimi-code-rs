//! YOLO-permission-mode approval policy.
//!
//! Original: `agent/permissionPolicy/policies/yolo-mode-approve.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_mode::AgentPermissionModeServiceContract,
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
};

pub struct YoloModeApprovePermissionPolicy {
    mode: Arc<dyn AgentPermissionModeServiceContract>,
}

impl YoloModeApprovePermissionPolicy {
    pub fn new(mode: Arc<dyn AgentPermissionModeServiceContract>) -> Self {
        Self { mode }
    }
}

fn yolo_mode_result(
    mode: crate::agent::permission_policy::PermissionMode,
) -> Option<PermissionPolicyResult> {
    (mode == crate::agent::permission_policy::PermissionMode::Yolo).then_some(
        PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        },
    )
}

impl PermissionPolicy for YoloModeApprovePermissionPolicy {
    fn name(&self) -> &str {
        "yolo-mode-approve"
    }

    fn evaluate<'a>(
        &'a self,
        _context: &'a crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { yolo_mode_result(self.mode.mode()) }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::permission_policy::PermissionMode;

    #[test]
    fn only_yolo_mode_approves() {
        assert!(matches!(
            yolo_mode_result(PermissionMode::Yolo),
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(yolo_mode_result(PermissionMode::Manual).is_none());
        assert!(yolo_mode_result(PermissionMode::Auto).is_none());
    }
}
