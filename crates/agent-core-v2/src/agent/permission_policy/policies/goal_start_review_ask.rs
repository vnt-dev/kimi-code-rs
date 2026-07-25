//! Requests a goal-start mode review outside automatic permission mode.
//!
//! Original: `agent/permissionPolicy/policies/goal-start-review-ask.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_mode::AgentPermissionModeServiceContract,
        permission_policy::{
            PermissionMode, PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult,
        },
        tool_executor::ResolvedToolExecutionHookContext,
    },
    session::approval::ApprovalDecision,
    tool::ToolInputDisplay,
};

pub struct GoalStartReviewAskPermissionPolicy {
    mode: Arc<dyn AgentPermissionModeServiceContract>,
}

impl GoalStartReviewAskPermissionPolicy {
    pub fn new(mode: Arc<dyn AgentPermissionModeServiceContract>) -> Self {
        Self { mode }
    }
}

pub fn to_permission_mode(label: Option<&str>) -> Option<PermissionMode> {
    match label {
        Some("auto") => Some(PermissionMode::Auto),
        Some("yolo") => Some(PermissionMode::Yolo),
        Some("manual") => Some(PermissionMode::Manual),
        _ => None,
    }
}

impl PermissionPolicy for GoalStartReviewAskPermissionPolicy {
    fn name(&self) -> &str {
        "goal-start-review-ask"
    }

    // Original: GoalStartReviewAskPermissionPolicyService.evaluate().
    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            if context.tool_call.name != "CreateGoal"
                || self.mode.mode() == PermissionMode::Auto
                || !matches!(
                    context.execution.display,
                    Some(ToolInputDisplay::GoalStart { .. })
                )
            {
                return None;
            }
            let mode = Arc::clone(&self.mode);
            Some(PermissionPolicyResult::Ask {
                reason: None,
                resolve_error: None,
                resolve_approval: Some(Box::new(move |result| {
                    if result.decision == ApprovalDecision::Approved
                        && let Some(next_mode) =
                            to_permission_mode(result.selected_label.as_deref())
                        && next_mode != mode.mode()
                    {
                        let _ = mode.set_mode(next_mode);
                    }
                    None
                })),
            })
        }
        .boxed()
    }
}
