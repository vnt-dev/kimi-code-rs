//! Agent RPC service contract.
//!
//! Original: `packages/agent-core-v2/src/agent/rpc/rpc.ts`.
//!
//! `PromisableMethods<AgentAPI>` becomes an async Rust trait. The two result
//! DTOs below preserve the concrete runtime values returned by the source
//! implementation, which are slightly richer than the declared `AgentAPI`
//! shapes (`ProfileData` and the computed tool `active` flag).

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    _base::di::instantiation::ServiceIdentifier,
    agent::{
        context_memory::AgentContextData,
        goal::{GoalSnapshot, GoalToolResult},
        permission_gate::PermissionData,
        plan::PlanData,
        profile::ProfileData,
        task::AgentTaskInfo,
        usage::UsageStatus,
    },
    session::todo::TodoItem,
    tool::ToolSource,
};

use super::core_api::*;

pub type AgentRpcError = Box<dyn Error + Send + Sync>;
pub type AgentRpcResult<T> = Result<T, AgentRpcError>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRpcToolInfo {
    pub name: String,
    pub description: String,
    pub active: bool,
    pub source: ToolSource,
}

#[async_trait]
pub trait AgentRpcServiceContract: Send + Sync {
    async fn prompt(&self, payload: PromptPayload) -> AgentRpcResult<PromptSubmitResult>;
    async fn run_shell_command(
        &self,
        payload: RunShellCommandPayload,
    ) -> AgentRpcResult<ShellCommandResult>;
    async fn cancel_shell_command(&self, payload: CancelShellCommandPayload) -> AgentRpcResult<()>;
    async fn steer(&self, payload: SteerPayload) -> AgentRpcResult<PromptSubmitResult>;
    async fn cancel(&self, payload: CancelPayload) -> AgentRpcResult<()>;
    async fn undo_history(&self, payload: UndoHistoryPayload) -> AgentRpcResult<u64>;
    async fn set_thinking(&self, payload: SetThinkingPayload) -> AgentRpcResult<()>;
    async fn set_permission(&self, payload: SetPermissionPayload) -> AgentRpcResult<()>;
    async fn set_model(&self, payload: SetModelPayload) -> AgentRpcResult<SetModelResult>;
    async fn get_model(&self, payload: EmptyPayload) -> AgentRpcResult<String>;
    async fn enter_plan(&self, payload: EmptyPayload) -> AgentRpcResult<()>;
    async fn cancel_plan(&self, payload: CancelPlanPayload) -> AgentRpcResult<()>;
    async fn clear_plan(&self, payload: EmptyPayload) -> AgentRpcResult<()>;
    async fn enter_swarm(&self, payload: EnterSwarmPayload) -> AgentRpcResult<()>;
    async fn exit_swarm(&self, payload: EmptyPayload) -> AgentRpcResult<()>;
    async fn get_swarm_mode(&self, payload: EmptyPayload) -> AgentRpcResult<bool>;
    async fn start_btw(&self, payload: EmptyPayload) -> AgentRpcResult<String>;
    async fn begin_compaction(&self, payload: BeginCompactionPayload) -> AgentRpcResult<()>;
    async fn cancel_compaction(&self, payload: EmptyPayload) -> AgentRpcResult<()>;
    async fn register_tool(&self, payload: RegisterToolPayload) -> AgentRpcResult<()>;
    async fn unregister_tool(&self, payload: UnregisterToolPayload) -> AgentRpcResult<()>;
    async fn set_active_tools(&self, payload: SetActiveToolsPayload) -> AgentRpcResult<()>;
    async fn stop_task(&self, payload: StopTaskPayload) -> AgentRpcResult<()>;
    async fn detach_task(
        &self,
        payload: DetachTaskPayload,
    ) -> AgentRpcResult<Option<AgentTaskInfo>>;
    async fn clear_context(&self, payload: EmptyPayload) -> AgentRpcResult<()>;
    async fn activate_skill(&self, payload: ActivateSkillPayload) -> AgentRpcResult<()>;
    async fn activate_plugin_command(
        &self,
        payload: ActivatePluginCommandPayload,
    ) -> AgentRpcResult<()>;
    async fn create_goal(&self, payload: CreateGoalPayload) -> AgentRpcResult<GoalSnapshot>;
    async fn get_goal(&self, payload: EmptyPayload) -> AgentRpcResult<GoalToolResult>;
    async fn pause_goal(&self, payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot>;
    async fn resume_goal(&self, payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot>;
    async fn cancel_goal(&self, payload: EmptyPayload) -> AgentRpcResult<GoalSnapshot>;
    async fn get_task_output(&self, payload: GetTaskOutputPayload) -> AgentRpcResult<String>;
    async fn get_context(&self, payload: EmptyPayload) -> AgentRpcResult<AgentContextData>;
    async fn get_config(&self, payload: EmptyPayload) -> AgentRpcResult<ProfileData>;
    async fn get_permission(&self, payload: EmptyPayload) -> AgentRpcResult<PermissionData>;
    async fn get_plan(&self, payload: EmptyPayload) -> AgentRpcResult<Option<PlanData>>;
    async fn get_todos(&self, payload: EmptyPayload) -> AgentRpcResult<Vec<TodoItem>>;
    async fn get_usage(&self, payload: EmptyPayload) -> AgentRpcResult<UsageStatus>;
    async fn get_tools(&self, payload: EmptyPayload) -> AgentRpcResult<Vec<AgentRpcToolInfo>>;
    async fn get_tasks(&self, payload: GetTasksPayload) -> AgentRpcResult<Vec<AgentTaskInfo>>;
}

#[derive(Clone)]
pub struct AgentRpcServiceHandle(pub Arc<dyn AgentRpcServiceContract>);

impl Deref for AgentRpcServiceHandle {
    type Target = dyn AgentRpcServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_RPC_SERVICE_ID: ServiceIdentifier<AgentRpcServiceHandle> =
    ServiceIdentifier::new("agentRPCService");

// Original `ISessionRPCService` identity. Its implementation belongs to the
// separate session RPC migration.
pub const AGENT_SESSION_RPC_SERVICE_ID: ServiceIdentifier<()> =
    ServiceIdentifier::new("agentSessionRPCService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identities_match_source_decorators() {
        assert_eq!(AGENT_RPC_SERVICE_ID.to_string(), "agentRPCService");
        assert_eq!(
            AGENT_SESSION_RPC_SERVICE_ID.to_string(),
            "agentSessionRPCService"
        );
    }

    #[test]
    fn concrete_tool_result_contains_computed_active_state() {
        assert_eq!(
            serde_json::to_value(AgentRpcToolInfo {
                name: "Bash".into(),
                description: "Run a command".into(),
                active: true,
                source: ToolSource::Builtin,
            })
            .unwrap(),
            serde_json::json!({
                "name": "Bash",
                "description": "Run a command",
                "active": true,
                "source": "builtin"
            })
        );
    }
}
