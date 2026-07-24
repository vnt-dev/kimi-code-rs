//! Ask before direct or discovered Git control-directory access.
//!
//! Original: `agent/permissionPolicy/policies/git-control-path-access-ask.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
        tool_executor::ResolvedToolExecutionHookContext,
    },
    os::interface::host_environment::{HostEnvironment, PathClass},
    session::workspace_context::SessionWorkspaceContextContract,
};

use super::{
    file_accesses, find_local_git_work_tree_marker, has_git_path_component, is_git_control_path,
};

pub struct GitControlPathAccessAskPermissionPolicy {
    environment: Arc<dyn HostEnvironment>,
    workspace: Arc<dyn SessionWorkspaceContextContract>,
}

impl GitControlPathAccessAskPermissionPolicy {
    pub fn new(
        environment: Arc<dyn HostEnvironment>,
        workspace: Arc<dyn SessionWorkspaceContextContract>,
    ) -> Self {
        Self {
            environment,
            workspace,
        }
    }
}

// Original: GitControlPathAccessAskPermissionPolicyService.evaluate().
async fn git_control_path_access_ask(
    context: &ResolvedToolExecutionHookContext,
    path_class: PathClass,
    cwd: &str,
) -> Option<PermissionPolicyResult> {
    if cwd.is_empty() {
        return None;
    }
    let accesses = file_accesses(context);
    if accesses.is_empty() {
        return None;
    }
    if accesses
        .iter()
        .any(|access| has_git_path_component(&access.path, cwd, path_class))
    {
        return Some(PermissionPolicyResult::Ask {
            reason: None,
            resolve_approval: None,
            resolve_error: None,
        });
    }

    let marker = find_local_git_work_tree_marker(cwd).await?;
    accesses
        .iter()
        .any(|access| is_git_control_path(&access.path, &marker, path_class))
        .then_some(PermissionPolicyResult::Ask {
            reason: None,
            resolve_approval: None,
            resolve_error: None,
        })
}

impl PermissionPolicy for GitControlPathAccessAskPermissionPolicy {
    fn name(&self) -> &str {
        "git-control-path-access-ask"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            // The policy cannot surface an environment-readiness error, and
            // lack of a verified path style must never result in approval.
            let path_class = self.environment.path_class().ok()?;
            let cwd = self.workspace.work_dir().to_string_lossy().into_owned();
            git_control_path_access_ask(context, path_class, &cwd).await
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use super::*;
    use crate::{
        _base::utils::abort::AbortController,
        agent::tool_executor::ToolExecutionHookContext,
        kosong::contract::message::{ToolCall, ToolCallType},
        tool::{ExecutableToolResult, RunnableToolExecution, ToolAccess, ToolExecute},
    };

    fn context(path: impl Into<String>) -> ResolvedToolExecutionHookContext {
        let execute: ToolExecute =
            Arc::new(|_| Box::pin(async { ExecutableToolResult::success("unused") }));
        let mut execution = RunnableToolExecution::new("read", execute);
        execution.accesses = Some(ToolAccess::read_file(path));
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

    fn temporary_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "kimi-git-control-policy-{label}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn asks_for_a_direct_dot_git_path_without_needing_a_marker_probe() {
        let root = temporary_root("direct");
        fs::create_dir_all(&root).unwrap();
        let cwd = root.to_string_lossy().into_owned();

        assert!(matches!(
            git_control_path_access_ask(
                &context(root.join(".git/config").to_string_lossy()),
                PathClass::Posix,
                &cwd,
            )
            .await,
            Some(PermissionPolicyResult::Ask { .. })
        ));
        assert!(
            git_control_path_access_ask(
                &context(root.join("src/lib.rs").to_string_lossy()),
                PathClass::Posix,
                &cwd,
            )
            .await
            .is_none()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn asks_for_a_discovered_linked_worktree_control_directory() {
        let root = temporary_root("worktree");
        let control = root.parent().unwrap().join("kimi-git-control-external");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".git"), "gitdir: ../kimi-git-control-external\n").unwrap();
        let cwd = root.to_string_lossy().into_owned();

        assert!(matches!(
            git_control_path_access_ask(
                &context(control.join("config").to_string_lossy()),
                PathClass::Posix,
                &cwd,
            )
            .await,
            Some(PermissionPolicyResult::Ask { .. })
        ));

        fs::remove_dir_all(root).unwrap();
    }
}
