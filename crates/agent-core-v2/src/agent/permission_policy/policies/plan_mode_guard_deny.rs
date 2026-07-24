//! Denials for mutating operations while a plan is active.
//!
//! Original: `agent/permissionPolicy/policies/plan-mode-guard-deny.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::agent::{
    permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
    plan::{AgentPlanServiceContract, PlanData},
    tool_executor::ResolvedToolExecutionHookContext,
};

use super::writes_only_plan_file;

pub struct PlanModeGuardDenyPermissionPolicy {
    plan: Arc<dyn AgentPlanServiceContract>,
}

impl PlanModeGuardDenyPermissionPolicy {
    pub fn new(plan: Arc<dyn AgentPlanServiceContract>) -> Self {
        Self { plan }
    }
}

pub fn plan_mode_write_denied_message(plan_file_path: Option<&str>) -> String {
    format!(
        "Plan mode is active. You may only write to the current plan file: {}. Call ExitPlanMode to exit plan mode before editing other files.",
        plan_file_path.unwrap_or("(no plan file selected yet)")
    )
}

// Original: PlanModeGuardDenyPermissionPolicyService.evaluate().
fn plan_mode_guard_denial(
    context: &ResolvedToolExecutionHookContext,
    plan: Option<&PlanData>,
) -> Option<PermissionPolicyResult> {
    let plan = plan?;
    let tool_name = context.tool_call.name.as_str();
    let message = if matches!(tool_name, "Write" | "Edit") {
        if writes_only_plan_file(context, &plan.path) {
            return None;
        }
        plan_mode_write_denied_message(Some(&plan.path))
    } else if tool_name == "TaskStop" {
        "TaskStop is not available in plan mode. Call ExitPlanMode to exit plan mode before stopping a background task.".into()
    } else if matches!(tool_name, "CronCreate" | "CronDelete") {
        format!(
            "{tool_name} is not available in plan mode because it would mutate scheduled work that runs after plan exit. Call ExitPlanMode first."
        )
    } else {
        return None;
    };
    Some(PermissionPolicyResult::Deny {
        reason: None,
        message: Some(message),
    })
}

impl PermissionPolicy for PlanModeGuardDenyPermissionPolicy {
    fn name(&self) -> &str {
        "plan-mode-guard-deny"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            let plan = self.plan.status().await.ok()?;
            plan_mode_guard_denial(context, plan.as_ref())
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::{
            permission_policy::policies::plan_mode_tool_approve::plan_mode_tool_approval,
            tool_executor::ToolExecutionHookContext,
        },
        kosong::contract::message::{ToolCall, ToolCallType},
        tool::{ExecutableToolResult, RunnableToolExecution, ToolAccess, ToolExecute},
    };

    fn context(tool_name: &str, access_path: Option<&str>) -> ResolvedToolExecutionHookContext {
        let execute: ToolExecute =
            Arc::new(|_| Box::pin(async { ExecutableToolResult::success("unused") }));
        let mut execution = RunnableToolExecution::new("rule", execute);
        execution.accesses = access_path.map(ToolAccess::write_file);
        ResolvedToolExecutionHookContext::new(
            ToolExecutionHookContext {
                turn_id: 1,
                signal: AbortController::new().signal(),
                trace: None,
                tool_call: ToolCall {
                    call_type: ToolCallType::Function,
                    id: "call-1".into(),
                    name: tool_name.into(),
                    arguments: None,
                    extras: None,
                    stream_index: None,
                },
                tool_calls: Vec::new(),
                tool: None,
                args: serde_json::Value::Null,
            },
            execution,
        )
    }

    fn active_plan() -> PlanData {
        PlanData {
            id: "p".into(),
            content: String::new(),
            path: "/plans/p.md".into(),
        }
    }

    #[test]
    fn allows_only_plan_file_writes_and_denies_other_plan_mode_mutations() {
        let plan = active_plan();
        let plan_write = context("Write", Some("/plans/p.md"));
        let other_write = context("Edit", Some("/repo/src.rs"));
        let cron = context("CronCreate", None);

        assert!(matches!(
            plan_mode_tool_approval(&context("EnterPlanMode", None), None),
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(matches!(
            plan_mode_tool_approval(&plan_write, Some(&plan)),
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(plan_mode_guard_denial(&plan_write, Some(&plan)).is_none());
        assert!(matches!(
            plan_mode_guard_denial(&other_write, Some(&plan)),
            Some(PermissionPolicyResult::Deny { .. })
        ));
        assert!(matches!(
            plan_mode_guard_denial(&cron, Some(&plan)),
            Some(PermissionPolicyResult::Deny { .. })
        ));
    }

    #[test]
    fn retains_source_plan_mode_denial_messages() {
        assert_eq!(
            plan_mode_write_denied_message(None),
            "Plan mode is active. You may only write to the current plan file: (no plan file selected yet). Call ExitPlanMode to exit plan mode before editing other files."
        );
        let plan = active_plan();
        let task_stop = context("TaskStop", None);
        let denied = plan_mode_guard_denial(&task_stop, Some(&plan));
        assert!(
            matches!(denied, Some(PermissionPolicyResult::Deny { message: Some(message), .. }) if message.contains("TaskStop is not available in plan mode"))
        );
    }
}
