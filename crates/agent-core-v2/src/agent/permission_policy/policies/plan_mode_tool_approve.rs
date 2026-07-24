//! Approval exceptions while entering, editing, and leaving plan mode.
//!
//! Original: `agent/permissionPolicy/policies/plan-mode-tool-approve.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
        plan::{AgentPlanServiceContract, PlanData},
        tool_executor::ResolvedToolExecutionHookContext,
    },
    tool::ToolInputDisplay,
};

use super::writes_only_plan_file;

pub struct PlanModeToolApprovePermissionPolicy {
    plan: Arc<dyn AgentPlanServiceContract>,
}

impl PlanModeToolApprovePermissionPolicy {
    pub fn new(plan: Arc<dyn AgentPlanServiceContract>) -> Self {
        Self { plan }
    }
}

// Original: PlanModeToolApprovePermissionPolicyService.evaluate().
pub(crate) fn plan_mode_tool_approval(
    context: &ResolvedToolExecutionHookContext,
    plan: Option<&PlanData>,
) -> Option<PermissionPolicyResult> {
    let tool_name = context.tool_call.name.as_str();
    if tool_name == "EnterPlanMode" {
        return Some(PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        });
    }
    if matches!(tool_name, "Write" | "Edit")
        && plan.is_some_and(|plan| writes_only_plan_file(context, &plan.path))
    {
        return Some(PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        });
    }
    if tool_name == "ExitPlanMode"
        && (plan.is_none()
            || !matches!(
                &context.execution.display,
                Some(ToolInputDisplay::PlanReview { .. })
            )
            || matches!(
                &context.execution.display,
                Some(ToolInputDisplay::PlanReview { plan, .. }) if plan.trim().is_empty()
            ))
    {
        return Some(PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        });
    }
    None
}

impl PermissionPolicy for PlanModeToolApprovePermissionPolicy {
    fn name(&self) -> &str {
        "plan-mode-tool-approve"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            if context.tool_call.name == "EnterPlanMode" {
                return plan_mode_tool_approval(context, None);
            }
            let plan = self.plan.status().await.ok()?;
            plan_mode_tool_approval(context, plan.as_ref())
        }
        .boxed()
    }
}
