//! Default approval whitelist for read-only and orchestration tools.
//!
//! Original: `agent/permissionPolicy/policies/default-tool-approve.ts`.

use std::{collections::BTreeSet, sync::LazyLock};

use futures_util::FutureExt;

use crate::agent::permission_policy::{
    PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult,
};

pub static DEFAULT_APPROVE_TOOLS: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    [
        "Read",
        "Grep",
        "Glob",
        "ReadMediaFile",
        "SetTodoList",
        "TodoList",
        "TaskList",
        "TaskOutput",
        "CronList",
        "WebSearch",
        "FetchURL",
        "Agent",
        "AskUserQuestion",
        "Skill",
        "GetGoal",
        "SetGoalBudget",
        "UpdateGoal",
        "select_tools",
    ]
    .into_iter()
    .collect()
});

pub fn is_default_approve_tool(tool_name: &str) -> bool {
    DEFAULT_APPROVE_TOOLS.contains(tool_name)
}

#[derive(Default)]
pub struct DefaultToolApprovePermissionPolicy;

impl PermissionPolicy for DefaultToolApprovePermissionPolicy {
    fn name(&self) -> &str {
        "default-tool-approve"
    }

    // Original: DefaultToolApprovePermissionPolicyService.evaluate().
    fn evaluate<'a>(
        &'a self,
        context: &'a crate::agent::tool_executor::ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            is_default_approve_tool(&context.tool_call.name).then_some(
                PermissionPolicyResult::Approve {
                    reason: None,
                    execution_metadata: None,
                },
            )
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelist_keeps_every_source_tool_name_and_excludes_mutating_tools() {
        assert_eq!(
            DefaultToolApprovePermissionPolicy.name(),
            "default-tool-approve"
        );
        for tool in ["Read", "AskUserQuestion", "CronList", "select_tools"] {
            assert!(is_default_approve_tool(tool));
        }
        for tool in ["Write", "Edit", "Bash"] {
            assert!(!is_default_approve_tool(tool));
        }
    }
}
