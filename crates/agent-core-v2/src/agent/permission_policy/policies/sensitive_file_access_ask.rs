//! Ask before accessing files classified as sensitive by the workspace policy.
//!
//! Original: `agent/permissionPolicy/policies/sensitive-file-access-ask.ts`.

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
        tool_executor::ResolvedToolExecutionHookContext,
    },
    tool::path_access::is_sensitive_file,
};

use super::file_accesses;

// Original: SensitiveFileAccessAskPermissionPolicyService.evaluate().
pub fn sensitive_file_access_ask(
    context: &ResolvedToolExecutionHookContext,
) -> Option<PermissionPolicyResult> {
    file_accesses(context)
        .iter()
        .any(|access| is_sensitive_file(&access.path))
        .then_some(PermissionPolicyResult::Ask {
            reason: None,
            resolve_approval: None,
            resolve_error: None,
        })
}

#[derive(Default)]
pub struct SensitiveFileAccessAskPermissionPolicy;

impl PermissionPolicy for SensitiveFileAccessAskPermissionPolicy {
    fn name(&self) -> &str {
        "sensitive-file-access-ask"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move { sensitive_file_access_ask(context) }.boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::tool_executor::ToolExecutionHookContext,
        kosong::contract::message::{ToolCall, ToolCallType},
        tool::{ExecutableToolResult, RunnableToolExecution, ToolAccesses, ToolExecute},
    };

    fn context(accesses: ToolAccesses) -> ResolvedToolExecutionHookContext {
        let execute: ToolExecute =
            Arc::new(|_| Box::pin(async { ExecutableToolResult::success("unused") }));
        let mut execution = RunnableToolExecution::new("read", execute);
        execution.accesses = Some(accesses);
        ResolvedToolExecutionHookContext::new(
            ToolExecutionHookContext {
                turn_id: 1,
                signal: AbortController::new().signal(),
                trace: None,
                tool_call: ToolCall {
                    call_type: ToolCallType::Function,
                    id: "call-1".into(),
                    name: "Read".into(),
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

    #[test]
    fn asks_for_sensitive_file_accesses_regardless_of_operation() {
        let read_secret = context(crate::tool::ToolAccess::read_file("/repo/.env"));
        let write_key = context(crate::tool::ToolAccess::write_file("/repo/id_ed25519"));
        let safe = context(crate::tool::ToolAccess::read_file("/repo/.env.example"));

        assert!(matches!(
            sensitive_file_access_ask(&read_secret),
            Some(PermissionPolicyResult::Ask { .. })
        ));
        assert!(matches!(
            sensitive_file_access_ask(&write_key),
            Some(PermissionPolicyResult::Ask { .. })
        ));
        assert!(sensitive_file_access_ask(&safe).is_none());
    }
}
