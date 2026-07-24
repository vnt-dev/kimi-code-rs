//! Automatic approval for workspace-local writes in a Git working tree.
//!
//! Original: `agent/permissionPolicy/policies/git-cwd-write-approve.ts`.

use std::sync::Arc;

use futures_util::FutureExt;

use crate::{
    agent::{
        permission_policy::{PermissionPolicy, PermissionPolicyFuture, PermissionPolicyResult},
        tool_executor::ResolvedToolExecutionHookContext,
    },
    os::interface::host_environment::{HostEnvironment, PathClass},
    session::workspace_context::SessionWorkspaceContextContract,
    tool::path_access::{WorkspaceConfig, is_within_workspace},
};

use super::{find_local_git_work_tree_marker, write_file_accesses};

pub struct GitCwdWriteApprovePermissionPolicy {
    environment: Arc<dyn HostEnvironment>,
    workspace: Arc<dyn SessionWorkspaceContextContract>,
}

impl GitCwdWriteApprovePermissionPolicy {
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

// Original: GitCwdWriteApprovePermissionPolicyService.evaluate().
async fn evaluate_git_cwd_write(
    context: &ResolvedToolExecutionHookContext,
    path_class: PathClass,
    cwd: &str,
    additional_dirs: &[String],
) -> Option<PermissionPolicyResult> {
    if !matches!(context.tool_call.name.as_str(), "Write" | "Edit")
        || path_class != PathClass::Posix
        || cwd.is_empty()
    {
        return None;
    }

    let write_accesses = write_file_accesses(context);
    if write_accesses.is_empty() {
        return None;
    }
    let workspace = WorkspaceConfig {
        workspace_dir: cwd.to_owned(),
        additional_dirs: additional_dirs.to_vec(),
    };
    if !write_accesses
        .iter()
        .all(|access| is_within_workspace(&access.path, &workspace, PathClass::Posix))
    {
        return None;
    }

    find_local_git_work_tree_marker(cwd)
        .await
        .map(|_| PermissionPolicyResult::Approve {
            reason: None,
            execution_metadata: None,
        })
}

impl PermissionPolicy for GitCwdWriteApprovePermissionPolicy {
    fn name(&self) -> &str {
        "git-cwd-write-approve"
    }

    fn evaluate<'a>(
        &'a self,
        context: &'a ResolvedToolExecutionHookContext,
    ) -> PermissionPolicyFuture<'a> {
        async move {
            // The source composes this policy only after the environment has
            // been initialized. The policy contract has no error channel, so
            // an unavailable host fact cannot grant approval.
            let path_class = self.environment.path_class().ok()?;
            let cwd = self.workspace.work_dir().to_string_lossy().into_owned();
            let additional_dirs = self
                .workspace
                .additional_dirs()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            evaluate_git_cwd_write(context, path_class, &cwd, &additional_dirs).await
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

    fn context(
        tool_name: &str,
        accesses: Option<crate::tool::ToolAccesses>,
    ) -> ResolvedToolExecutionHookContext {
        let execute: ToolExecute =
            Arc::new(|_| Box::pin(async { ExecutableToolResult::success("unused") }));
        let mut execution = RunnableToolExecution::new("write", execute);
        execution.accesses = accesses;
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

    fn git_workspace(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "kimi-git-cwd-write-policy-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join(".git")).unwrap();
        root
    }

    #[tokio::test]
    async fn approves_write_and_edit_only_inside_a_posix_git_workspace() {
        let root = git_workspace("approved");
        let cwd = root.to_string_lossy().into_owned();
        let write = context(
            "Write",
            Some(ToolAccess::write_file(
                root.join("src/lib.rs").to_string_lossy(),
            )),
        );
        let edit = context(
            "Edit",
            Some(ToolAccess::write_file(
                root.join("README.md").to_string_lossy(),
            )),
        );

        assert!(matches!(
            evaluate_git_cwd_write(&write, PathClass::Posix, &cwd, &[]).await,
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(matches!(
            evaluate_git_cwd_write(&edit, PathClass::Posix, &cwd, &[]).await,
            Some(PermissionPolicyResult::Approve { .. })
        ));
        assert!(
            evaluate_git_cwd_write(&write, PathClass::Win32, &cwd, &[])
                .await
                .is_none()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn does_not_approve_non_writes_outside_paths_or_non_git_workspaces() {
        let root = git_workspace("denied");
        let cwd = root.to_string_lossy().into_owned();
        let read = context(
            "Read",
            Some(ToolAccess::write_file(root.join("a.txt").to_string_lossy())),
        );
        let outside = context("Write", Some(ToolAccess::write_file("/outside/a.txt")));
        let no_accesses = context("Write", None);
        let no_git = std::env::temp_dir().join(format!("kimi-no-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&no_git).unwrap();

        assert!(
            evaluate_git_cwd_write(&read, PathClass::Posix, &cwd, &[])
                .await
                .is_none()
        );
        assert!(
            evaluate_git_cwd_write(&outside, PathClass::Posix, &cwd, &[])
                .await
                .is_none()
        );
        assert!(
            evaluate_git_cwd_write(&no_accesses, PathClass::Posix, &cwd, &[])
                .await
                .is_none()
        );
        assert!(
            evaluate_git_cwd_write(
                &context(
                    "Write",
                    Some(ToolAccess::write_file(
                        no_git.join("a.txt").to_string_lossy()
                    ))
                ),
                PathClass::Posix,
                &no_git.to_string_lossy(),
                &[],
            )
            .await
            .is_none()
        );

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(no_git).unwrap();
    }
}
