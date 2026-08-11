//! Native RPC request, response, and API contracts.
//!
//! Original: `packages/agent-core-v2/src/agent/rpc/core-api.ts`.
//!
//! The three API traits preserve the source layering. [`AgentApi`] describes
//! agent-local methods, [`SessionApi`] adds session-local methods and an agent
//! identity, and [`CoreApi`] adds app-level methods and a session identity.
//! Async adaptation belongs to the RPC service layer, matching the source's
//! separate `PromisableMethods` wrapper.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

pub use crate::{
    agent::{
        goal::types::{
            GoalBudgetLimits, GoalBudgetReport, GoalChange, GoalChangeStats, GoalSnapshot,
            GoalStatus, GoalToolResult,
        },
        profile::{AgentConfigData, AgentConfigUpdateData},
    },
    app::{
        plugin::{PluginCommandDef, PluginInfo, PluginSummary, ReloadSummary},
        session_export::{
            ExportSessionManifest, ExportSessionPayload, ExportSessionResult, ShellEnvironment,
        },
    },
};

use crate::{
    agent::{
        context_memory::{AgentContextData, ContextMessage},
        full_compaction::CompactionResult,
        mcp::McpServerConfig,
        permission_gate::PermissionData,
        permission_policy::PermissionMode,
        permission_rules::PermissionApprovalResultRecord,
        plan::PlanData,
        swarm::SwarmModeTrigger,
        task::AgentTaskInfo,
        usage::UsageStatus,
    },
    app::{config::ResolvedConfig, flag::ExperimentalFeatureState, session_legacy::SessionWarning},
    kosong::contract::message::{ContentPart, MediaUrl},
    session::{session_metadata::SessionMeta, todo::TodoItem},
    tool::ToolInfo,
};

pub type JsonValue = Value;
pub type JsonObject = Map<String, Value>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonPrimitive {
    String(String),
    Number(Number),
    Boolean(bool),
    Null,
}

pub type Unsubscribe = Box<dyn FnOnce() + 'static>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum TextPromptPart {
    #[serde(rename = "text")]
    Text { text: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum PromptPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl {
        #[serde(rename = "imageUrl")]
        image_url: MediaUrl,
    },
    #[serde(rename = "video_url")]
    VideoUrl {
        #[serde(rename = "videoUrl")]
        video_url: MediaUrl,
    },
}

pub type PromptInput = Vec<PromptPart>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum PromptFilePart {
    #[serde(rename = "file")]
    File {
        file_id: String,
        name: String,
        media_type: String,
        size: u64,
    },
}

/// Prompt wire input accepts the provider-neutral content parts used today
/// plus an uploaded-file reference. File parts are resolved before they enter
/// the model/provider message contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PromptInputPart {
    Content(ContentPart),
    File(PromptFilePart),
}

impl From<ContentPart> for PromptInputPart {
    fn from(value: ContentPart) -> Self {
        Self::Content(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyPayload {}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadataPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_custom_title: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<BTreeMap<String, Value>>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientTelemetryInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub work_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<BTreeMap<String, McpServerConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientTelemetryInfo>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionPayload {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveSessionPayload {
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSessionPayload {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<BTreeMap<String, McpServerConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_dirs: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReloadSessionPayload {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_plugin_session_start_reminder: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionPayload {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_archive: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CoreInfo {
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    pub work_dir: String,
    pub session_dir: String,
    pub created_at: f64,
    pub updated_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_dirs: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSkillSelection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub input: Vec<PromptInputPart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<PromptSkillSelection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunShellCommandPayload {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backgrounded: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelShellCommandPayload {
    pub command_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    pub input: Vec<PromptInputPart>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetThinkingPayload {
    pub level: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetPermissionPayload {
    pub mode: PermissionMode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetModelPayload {
    pub model: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelResult {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CancelPlanPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnterSwarmPayload {
    pub trigger: SwarmModeTrigger,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BeginCompactionPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct UndoHistoryPayload {
    pub count: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RegisterToolPayload {
    pub name: String,
    pub description: String,
    pub parameters: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnregisterToolPayload {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetActiveToolsPayload {
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopTaskPayload {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachTaskPayload {
    pub task_id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTaskOutputPayload {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTasksPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSummarySource {
    Builtin,
    User,
    Extra,
    Project,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub path: String,
    pub source: SkillSummarySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_sub_skill: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivateSkillPayload {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivatePluginCommandPayload {
    pub plugin_id: String,
    pub command_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerTransport {
    Stdio,
    Http,
    Sse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerStatus {
    Pending,
    Connected,
    Failed,
    Disabled,
    NeedsAuth,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub transport: McpServerTransport,
    pub status: McpServerStatus,
    pub tool_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStartupMetrics {
    pub duration_ms: f64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReconnectMcpServerPayload {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstallPluginPayload {
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetPluginEnabledPayload {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SetPluginMcpServerEnabledPayload {
    pub id: String,
    pub server: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemovePluginPayload {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetPluginInfoPayload {
    pub id: String,
}

pub type ReloadPluginsResult = ReloadSummary;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AddAdditionalDirPayload {
    pub path: String,
    pub persist: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAdditionalDirResult {
    pub additional_dirs: Vec<String>,
    pub project_root: String,
    pub config_path: String,
    pub persisted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenameSessionPayload {
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UpdateSessionMetadataPayload {
    pub metadata: SessionMetadataPatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateGoalPayload {
    pub objective: String,
    #[serde(
        default,
        rename = "completionCriterion",
        skip_serializing_if = "Option::is_none"
    )]
    pub completion_criterion: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetKimiConfigPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfigDiagnostics {
    pub warnings: Vec<String>,
}

pub type SetKimiConfigPayload = ResolvedConfig;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveKimiProviderPayload {
    pub provider_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptSubmitStatus {
    Queued,
    Running,
    Steered,
    Completed,
    Failed,
    Cancelled,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSubmitResult {
    pub prompt_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    pub status: PromptSubmitStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithAgentId<T> {
    pub agent_id: String,
    #[serde(flatten)]
    pub value: T,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WithSessionId<T> {
    pub session_id: String,
    #[serde(flatten)]
    pub value: T,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResumedAgentType {
    Main,
    Sub,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CompactionReplayResult {
    Result(CompactionResult),
    Status(CompactionReplayStatus),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionReplayStatus {
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalReplayCreatedKind {
    Created,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoalReplayCreatedChange {
    pub kind: GoalReplayCreatedKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum GoalReplayChange {
    Change(GoalChange),
    Created(GoalReplayCreatedChange),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentReplayRecordPayload {
    Message {
        message: ContextMessage,
    },
    Compaction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<CompactionReplayResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instruction: Option<String>,
    },
    GoalUpdated {
        snapshot: GoalSnapshot,
        change: GoalReplayChange,
    },
    PlanUpdated {
        enabled: bool,
    },
    ConfigUpdated {
        config: AgentConfigUpdateData,
    },
    PermissionUpdated {
        mode: PermissionMode,
    },
    ApprovalResult {
        record: PermissionApprovalResultRecord,
    },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AgentReplayRecord {
    pub time: f64,
    #[serde(flatten)]
    pub payload: AgentReplayRecordPayload,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumedAgentState {
    pub r#type: ResumedAgentType,
    pub config: AgentConfigData,
    pub context: AgentContextData,
    pub replay: Vec<AgentReplayRecord>,
    pub permission: PermissionData,
    pub plan: Option<PlanData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_mode: Option<bool>,
    pub usage: UsageStatus,
    pub tools: Vec<ToolInfo>,
    pub tasks: Vec<AgentTaskInfo>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSessionResult {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub session_metadata: SessionMeta,
    pub agents: BTreeMap<String, ResumedAgentState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub trait AgentApi {
    fn prompt(&self, payload: PromptPayload) -> PromptSubmitResult;
    fn run_shell_command(&self, payload: RunShellCommandPayload) -> ShellCommandResult;
    fn cancel_shell_command(&self, payload: CancelShellCommandPayload);
    fn steer(&self, payload: SteerPayload) -> PromptSubmitResult;
    fn cancel(&self, payload: CancelPayload);
    fn undo_history(&self, payload: UndoHistoryPayload) -> u64;
    fn set_thinking(&self, payload: SetThinkingPayload);
    fn set_permission(&self, payload: SetPermissionPayload);
    fn set_model(&self, payload: SetModelPayload) -> SetModelResult;
    fn get_model(&self, payload: EmptyPayload) -> String;
    fn enter_plan(&self, payload: EmptyPayload);
    fn cancel_plan(&self, payload: CancelPlanPayload);
    fn clear_plan(&self, payload: EmptyPayload);
    fn enter_swarm(&self, payload: EnterSwarmPayload);
    fn exit_swarm(&self, payload: EmptyPayload);
    fn get_swarm_mode(&self, payload: EmptyPayload) -> bool;
    fn start_btw(&self, payload: EmptyPayload) -> String;
    fn begin_compaction(&self, payload: BeginCompactionPayload);
    fn cancel_compaction(&self, payload: EmptyPayload);
    fn register_tool(&self, payload: RegisterToolPayload);
    fn unregister_tool(&self, payload: UnregisterToolPayload);
    fn set_active_tools(&self, payload: SetActiveToolsPayload);
    fn stop_task(&self, payload: StopTaskPayload);
    fn detach_task(&self, payload: DetachTaskPayload) -> Option<AgentTaskInfo>;
    fn clear_context(&self, payload: EmptyPayload);
    fn activate_skill(&self, payload: ActivateSkillPayload);
    fn activate_plugin_command(&self, payload: ActivatePluginCommandPayload);
    fn create_goal(&self, payload: CreateGoalPayload) -> GoalSnapshot;
    fn get_goal(&self, payload: EmptyPayload) -> GoalToolResult;
    fn pause_goal(&self, payload: EmptyPayload) -> GoalSnapshot;
    fn resume_goal(&self, payload: EmptyPayload) -> GoalSnapshot;
    fn cancel_goal(&self, payload: EmptyPayload) -> GoalSnapshot;
    fn get_task_output(&self, payload: GetTaskOutputPayload) -> String;
    fn get_context(&self, payload: EmptyPayload) -> AgentContextData;
    fn get_config(&self, payload: EmptyPayload) -> AgentConfigData;
    fn get_permission(&self, payload: EmptyPayload) -> PermissionData;
    fn get_plan(&self, payload: EmptyPayload) -> Option<PlanData>;
    fn get_todos(&self, payload: EmptyPayload) -> Vec<TodoItem>;
    fn get_usage(&self, payload: EmptyPayload) -> UsageStatus;
    fn get_tools(&self, payload: EmptyPayload) -> Vec<ToolInfo>;
    fn get_tasks(&self, payload: GetTasksPayload) -> Vec<AgentTaskInfo>;
}

pub trait SessionApi: AgentApi {
    fn agent_id(&self) -> &str;
    fn rename_session(&self, payload: RenameSessionPayload);
    fn update_session_metadata(&self, payload: UpdateSessionMetadataPayload);
    fn get_session_metadata(&self, payload: EmptyPayload) -> SessionMeta;
    fn list_skills(&self, payload: EmptyPayload) -> Vec<SkillSummary>;
    fn list_plugin_commands(&self, payload: EmptyPayload) -> Vec<PluginCommandDef>;
    fn list_mcp_servers(&self, payload: EmptyPayload) -> Vec<McpServerInfo>;
    fn get_mcp_startup_metrics(&self, payload: EmptyPayload) -> McpStartupMetrics;
    fn reconnect_mcp_server(&self, payload: ReconnectMcpServerPayload);
    fn generate_agents_md(&self, payload: EmptyPayload);
    fn get_session_warnings(&self, payload: EmptyPayload) -> Vec<SessionWarning>;
    fn add_additional_dir(&self, payload: AddAdditionalDirPayload) -> AddAdditionalDirResult;
}

pub trait CoreApi: SessionApi {
    fn session_id(&self) -> &str;
    fn get_core_info(&self, payload: EmptyPayload) -> CoreInfo;
    fn get_experimental_features(&self, payload: EmptyPayload) -> Vec<ExperimentalFeatureState>;
    fn get_kimi_config(&self, payload: GetKimiConfigPayload) -> ResolvedConfig;
    fn get_config_diagnostics(&self, payload: EmptyPayload) -> ConfigDiagnostics;
    fn set_kimi_config(&self, payload: SetKimiConfigPayload) -> ResolvedConfig;
    fn remove_kimi_provider(&self, payload: RemoveKimiProviderPayload) -> ResolvedConfig;
    fn create_session(&self, payload: CreateSessionPayload) -> SessionSummary;
    fn close_session(&self, payload: CloseSessionPayload);
    fn archive_session(&self, payload: ArchiveSessionPayload);
    fn resume_session(&self, payload: ResumeSessionPayload) -> ResumeSessionResult;
    fn reload_session(&self, payload: ReloadSessionPayload) -> ResumeSessionResult;
    fn fork_session(&self, payload: ForkSessionPayload) -> ResumeSessionResult;
    fn list_sessions(&self, payload: ListSessionsPayload) -> Vec<SessionSummary>;
    fn export_session(&self, payload: ExportSessionPayload) -> ExportSessionResult;
    fn list_plugins(&self, payload: EmptyPayload) -> Vec<PluginSummary>;
    fn install_plugin(&self, payload: InstallPluginPayload) -> PluginSummary;
    fn set_plugin_enabled(&self, payload: SetPluginEnabledPayload);
    fn set_plugin_mcp_server_enabled(&self, payload: SetPluginMcpServerEnabledPayload);
    fn remove_plugin(&self, payload: RemovePluginPayload);
    fn reload_plugins(&self, payload: EmptyPayload) -> ReloadPluginsResult;
    fn get_plugin_info(&self, payload: GetPluginInfoPayload) -> PluginInfo;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn create_session_payload_preserves_optional_camel_case_wire_fields() {
        let payload = CreateSessionPayload {
            id: None,
            work_dir: "/repo".into(),
            model: Some("kimi".into()),
            thinking: None,
            permission: Some(PermissionMode::Auto),
            metadata: Some(Map::from_iter([("source".into(), json!("desktop"))])),
            mcp_servers: None,
            additional_dirs: Some(vec!["/shared".into()]),
            client: Some(ClientTelemetryInfo {
                name: Some("web".into()),
                ui_mode: Some("chat".into()),
                ..ClientTelemetryInfo::default()
            }),
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "workDir": "/repo",
                "model": "kimi",
                "permission": "auto",
                "metadata": {"source": "desktop"},
                "additionalDirs": ["/shared"],
                "client": {"name": "web", "uiMode": "chat"}
            })
        );
    }

    #[test]
    fn prompt_part_types_reject_non_prompt_content_and_preserve_media_shape() {
        let input: PromptInput = vec![
            PromptPart::Text {
                text: "hello".into(),
            },
            PromptPart::ImageUrl {
                image_url: MediaUrl {
                    url: "data:image/png;base64,AA==".into(),
                    id: Some("image-1".into()),
                },
            },
        ];

        assert_eq!(
            serde_json::to_value(input).unwrap(),
            json!([
                {"type": "text", "text": "hello"},
                {
                    "type": "image_url",
                    "imageUrl": {
                        "url": "data:image/png;base64,AA==",
                        "id": "image-1"
                    }
                }
            ])
        );
        assert!(
            serde_json::from_value::<PromptPart>(json!({
                "type": "audio_url",
                "audioUrl": {"url": "audio"}
            }))
            .is_err()
        );
    }

    #[test]
    fn prompt_payload_accepts_uploaded_file_parts_without_changing_content_part() {
        let payload = serde_json::from_value::<PromptPayload>(json!({
            "input": [
                {"type": "text", "text": "inspect this"},
                {
                    "type": "file",
                    "file_id": "f_1",
                    "name": "data.xlsx",
                    "media_type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                    "size": 42
                }
            ]
        }))
        .unwrap();

        assert!(matches!(
            &payload.input[0],
            PromptInputPart::Content(ContentPart::Text { text }) if text == "inspect this"
        ));
        assert!(matches!(
            &payload.input[1],
            PromptInputPart::File(PromptFilePart::File {
                file_id,
                name,
                size: 42,
                ..
            }) if file_id == "f_1" && name == "data.xlsx"
        ));
        assert_eq!(
            serde_json::to_value(payload).unwrap()["input"][1]["type"],
            "file"
        );

        let steer = serde_json::from_value::<SteerPayload>(json!({
            "promptId": "queued-local-1",
            "input": [
                {"type": "text", "text": "do this now"},
                {
                    "type": "file",
                    "file_id": "f_2",
                    "name": "notes.txt",
                    "media_type": "text/plain",
                    "size": 12
                }
            ]
        }))
        .unwrap();
        assert_eq!(steer.prompt_id.as_deref(), Some("queued-local-1"));
        assert!(matches!(
            &steer.input[1],
            PromptInputPart::File(PromptFilePart::File {
                file_id,
                name,
                size: 12,
                ..
            }) if file_id == "f_2" && name == "notes.txt"
        ));
    }

    #[test]
    fn identifier_wrappers_flatten_payload_fields() {
        let wrapped = WithSessionId {
            session_id: "session-1".into(),
            value: WithAgentId {
                agent_id: "main".into(),
                value: CancelPayload {
                    turn_id: Some(crate::agent::TurnId::new(7)),
                },
            },
        };

        assert_eq!(
            serde_json::to_value(wrapped).unwrap(),
            json!({
                "sessionId": "session-1",
                "agentId": "main",
                "turnId": 7
            })
        );
        assert_eq!(
            serde_json::to_value(PromptSubmitResult {
                prompt_id: "prompt-1".into(),
                turn_id: Some(crate::agent::TurnId::new(7)),
                status: PromptSubmitStatus::Running,
            })
            .unwrap(),
            json!({
                "promptId": "prompt-1",
                "turnId": 7,
                "status": "running"
            })
        );
    }

    #[test]
    fn prompt_payload_accepts_structured_skill_selections() {
        let payload = PromptPayload {
            prompt_id: None,
            input: vec![
                ContentPart::Text {
                    text: "review this".into(),
                }
                .into(),
            ],
            disabled_tools: None,
            skills: vec![
                PromptSkillSelection {
                    name: "review".into(),
                    args: Some("--strict".into()),
                },
                PromptSkillSelection {
                    name: "docs".into(),
                    args: None,
                },
            ],
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "input": [{"type": "text", "text": "review this"}],
                "skills": [
                    {"name": "review", "args": "--strict"},
                    {"name": "docs"}
                ]
            })
        );
    }

    #[test]
    fn session_metadata_patch_matches_partial_session_meta_without_agents() {
        let payload = UpdateSessionMetadataPayload {
            metadata: SessionMetadataPatch {
                id: Some("session-2".into()),
                created_at: Some(10),
                title: Some("Renamed".into()),
                custom: Some(BTreeMap::from([("owner".into(), json!("user"))])),
                ..SessionMetadataPatch::default()
            },
        };

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({
                "metadata": {
                    "id": "session-2",
                    "title": "Renamed",
                    "createdAt": 10,
                    "custom": {"owner": "user"}
                }
            })
        );
    }

    #[test]
    fn replay_union_preserves_source_discriminants() {
        let records = vec![
            AgentReplayRecord {
                time: 1.0,
                payload: AgentReplayRecordPayload::Compaction {
                    result: Some(CompactionReplayResult::Status(
                        CompactionReplayStatus::Cancelled,
                    )),
                    instruction: Some("keep decisions".into()),
                },
            },
            AgentReplayRecord {
                time: 2.0,
                payload: AgentReplayRecordPayload::PlanUpdated { enabled: true },
            },
        ];

        assert_eq!(
            serde_json::to_value(records).unwrap(),
            json!([
                {
                    "time": 1.0,
                    "type": "compaction",
                    "result": "cancelled",
                    "instruction": "keep decisions"
                },
                {
                    "time": 2.0,
                    "type": "plan_updated",
                    "enabled": true
                }
            ])
        );
    }

    #[test]
    fn empty_payload_is_an_empty_json_object() {
        assert_eq!(
            serde_json::to_value(EmptyPayload::default()).unwrap(),
            json!({})
        );
    }
}
