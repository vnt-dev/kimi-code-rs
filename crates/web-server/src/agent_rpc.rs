use std::error::Error;

use kimi_code_agent_core_v2::{
    _base::errors::errors::Error2,
    agent::rpc::*,
    app::{desktop_client::KimiCodeDesktopClient, file::FileError},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRpcMethod {
    Prompt,
    RunShellCommand,
    CancelShellCommand,
    Steer,
    Cancel,
    UndoHistory,
    SetThinking,
    SetPermission,
    RenameSession,
    GenerateConversationTitle,
    SetModel,
    GetModel,
    EnterPlan,
    CancelPlan,
    ClearPlan,
    EnterSwarm,
    ExitSwarm,
    GetSwarmMode,
    StartBtw,
    BeginCompaction,
    CancelCompaction,
    RegisterTool,
    UnregisterTool,
    SetActiveTools,
    StopTask,
    DetachTask,
    ClearContext,
    ActivateSkill,
    ListPluginCommands,
    ListMcpServers,
    ActivatePluginCommand,
    CreateGoal,
    GetGoal,
    PauseGoal,
    ResumeGoal,
    CancelGoal,
    GetTaskOutput,
    GetContext,
    GetConfig,
    GetPermission,
    GetPlan,
    GetTodos,
    GetUsage,
    GetTools,
    GetTasks,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRpcRequest {
    pub session_id: String,
    pub agent_id: String,
    pub method: AgentRpcMethod,
    #[serde(default = "empty_payload")]
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Map<String, Value>>,
}

impl RpcError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            code: "transport.error".into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: "request.invalid".into(),
            message: message.into(),
            details: None,
        }
    }

    fn invalid_payload(error: serde_json::Error) -> Self {
        Self::invalid_request(error.to_string())
    }

    fn core(error: &(dyn Error + 'static)) -> Self {
        let coded = error
            .downcast_ref::<Error2>()
            .or_else(|| error.downcast_ref::<FileError>().map(FileError::error));
        if let Some(error) = coded {
            Self {
                code: error.code.clone(),
                message: error.message.clone(),
                details: error.details.clone(),
            }
        } else {
            Self::transport(error.to_string())
        }
    }
}

fn empty_payload() -> Value {
    Value::Object(Default::default())
}

fn decode<T: DeserializeOwned>(payload: Value) -> Result<T, RpcError> {
    serde_json::from_value(payload).map_err(RpcError::invalid_payload)
}

fn encode<T: Serialize>(value: T) -> Result<Value, RpcError> {
    serde_json::to_value(value).map_err(|error| RpcError::transport(error.to_string()))
}

macro_rules! invoke {
    ($rpc:expr, $payload:expr, $method:ident, $payload_type:ty) => {{
        let payload = decode::<$payload_type>($payload)?;
        let result = $rpc
            .$method(payload)
            .await
            .map_err(|error| RpcError::core(error.as_ref()))?;
        encode(result)
    }};
}

pub async fn dispatch_agent_rpc(
    client: &KimiCodeDesktopClient,
    request: AgentRpcRequest,
) -> Result<Value, RpcError> {
    let rpc = client
        .agent_rpc(&request.session_id, &request.agent_id)
        .await
        .map_err(|error| RpcError::transport(error.to_string()))?;
    let payload = request.payload;

    match request.method {
        AgentRpcMethod::Prompt => invoke!(rpc, payload, prompt, PromptPayload),
        AgentRpcMethod::RunShellCommand => {
            invoke!(rpc, payload, run_shell_command, RunShellCommandPayload)
        }
        AgentRpcMethod::CancelShellCommand => {
            invoke!(
                rpc,
                payload,
                cancel_shell_command,
                CancelShellCommandPayload
            )
        }
        AgentRpcMethod::Steer => invoke!(rpc, payload, steer, SteerPayload),
        AgentRpcMethod::Cancel => invoke!(rpc, payload, cancel, CancelPayload),
        AgentRpcMethod::UndoHistory => invoke!(rpc, payload, undo_history, UndoHistoryPayload),
        AgentRpcMethod::SetThinking => invoke!(rpc, payload, set_thinking, SetThinkingPayload),
        AgentRpcMethod::SetPermission => {
            invoke!(rpc, payload, set_permission, SetPermissionPayload)
        }
        AgentRpcMethod::RenameSession => {
            invoke!(rpc, payload, rename_session, RenameSessionPayload)
        }
        AgentRpcMethod::GenerateConversationTitle => {
            invoke!(
                rpc,
                payload,
                generate_conversation_title,
                GenerateConversationTitlePayload
            )
        }
        AgentRpcMethod::SetModel => invoke!(rpc, payload, set_model, SetModelPayload),
        AgentRpcMethod::GetModel => invoke!(rpc, payload, get_model, EmptyPayload),
        AgentRpcMethod::EnterPlan => invoke!(rpc, payload, enter_plan, EmptyPayload),
        AgentRpcMethod::CancelPlan => invoke!(rpc, payload, cancel_plan, CancelPlanPayload),
        AgentRpcMethod::ClearPlan => invoke!(rpc, payload, clear_plan, EmptyPayload),
        AgentRpcMethod::EnterSwarm => invoke!(rpc, payload, enter_swarm, EnterSwarmPayload),
        AgentRpcMethod::ExitSwarm => invoke!(rpc, payload, exit_swarm, EmptyPayload),
        AgentRpcMethod::GetSwarmMode => invoke!(rpc, payload, get_swarm_mode, EmptyPayload),
        AgentRpcMethod::StartBtw => invoke!(rpc, payload, start_btw, EmptyPayload),
        AgentRpcMethod::BeginCompaction => {
            invoke!(rpc, payload, begin_compaction, BeginCompactionPayload)
        }
        AgentRpcMethod::CancelCompaction => invoke!(rpc, payload, cancel_compaction, EmptyPayload),
        AgentRpcMethod::RegisterTool => invoke!(rpc, payload, register_tool, RegisterToolPayload),
        AgentRpcMethod::UnregisterTool => {
            invoke!(rpc, payload, unregister_tool, UnregisterToolPayload)
        }
        AgentRpcMethod::SetActiveTools => {
            invoke!(rpc, payload, set_active_tools, SetActiveToolsPayload)
        }
        AgentRpcMethod::StopTask => invoke!(rpc, payload, stop_task, StopTaskPayload),
        AgentRpcMethod::DetachTask => invoke!(rpc, payload, detach_task, DetachTaskPayload),
        AgentRpcMethod::ClearContext => invoke!(rpc, payload, clear_context, EmptyPayload),
        AgentRpcMethod::ActivateSkill => {
            invoke!(rpc, payload, activate_skill, ActivateSkillPayload)
        }
        AgentRpcMethod::ListPluginCommands => {
            invoke!(rpc, payload, list_plugin_commands, EmptyPayload)
        }
        AgentRpcMethod::ListMcpServers => {
            invoke!(rpc, payload, list_mcp_servers, EmptyPayload)
        }
        AgentRpcMethod::ActivatePluginCommand => {
            invoke!(
                rpc,
                payload,
                activate_plugin_command,
                ActivatePluginCommandPayload
            )
        }
        AgentRpcMethod::CreateGoal => invoke!(rpc, payload, create_goal, CreateGoalPayload),
        AgentRpcMethod::GetGoal => invoke!(rpc, payload, get_goal, EmptyPayload),
        AgentRpcMethod::PauseGoal => invoke!(rpc, payload, pause_goal, EmptyPayload),
        AgentRpcMethod::ResumeGoal => invoke!(rpc, payload, resume_goal, EmptyPayload),
        AgentRpcMethod::CancelGoal => invoke!(rpc, payload, cancel_goal, EmptyPayload),
        AgentRpcMethod::GetTaskOutput => {
            invoke!(rpc, payload, get_task_output, GetTaskOutputPayload)
        }
        AgentRpcMethod::GetContext => invoke!(rpc, payload, get_context, EmptyPayload),
        AgentRpcMethod::GetConfig => invoke!(rpc, payload, get_config, EmptyPayload),
        AgentRpcMethod::GetPermission => invoke!(rpc, payload, get_permission, EmptyPayload),
        AgentRpcMethod::GetPlan => invoke!(rpc, payload, get_plan, EmptyPayload),
        AgentRpcMethod::GetTodos => invoke!(rpc, payload, get_todos, EmptyPayload),
        AgentRpcMethod::GetUsage => invoke!(rpc, payload, get_usage, EmptyPayload),
        AgentRpcMethod::GetTools => invoke!(rpc, payload, get_tools, EmptyPayload),
        AgentRpcMethod::GetTasks => invoke!(rpc, payload, get_tasks, GetTasksPayload),
    }
}

#[cfg(test)]
mod tests {
    use kimi_code_agent_core_v2::app::file::file_not_found_error;

    use super::{AgentRpcMethod, RpcError};

    #[test]
    fn rpc_method_names_match_the_typescript_facade() {
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"runShellCommand\"").unwrap(),
            AgentRpcMethod::RunShellCommand
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"startBtw\"").unwrap(),
            AgentRpcMethod::StartBtw
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"listPluginCommands\"").unwrap(),
            AgentRpcMethod::ListPluginCommands
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"listMcpServers\"").unwrap(),
            AgentRpcMethod::ListMcpServers
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"activatePluginCommand\"").unwrap(),
            AgentRpcMethod::ActivatePluginCommand
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"getTodos\"").unwrap(),
            AgentRpcMethod::GetTodos
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"renameSession\"").unwrap(),
            AgentRpcMethod::RenameSession
        ));
        assert!(matches!(
            serde_json::from_str::<AgentRpcMethod>("\"generateConversationTitle\"").unwrap(),
            AgentRpcMethod::GenerateConversationTitle
        ));
    }

    #[test]
    fn uploaded_file_errors_keep_their_domain_code() {
        let error = file_not_found_error("f_missing");
        let projected = RpcError::core(&error);
        assert_eq!(projected.code, "file.not_found");
        assert_eq!(projected.details.unwrap()["fileId"], "f_missing");
    }
}
