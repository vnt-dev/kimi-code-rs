use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

use super::MessageContent;
use super::display::{OptionalJsonValue, ToolInputDisplay};
use super::model_catalog::{ProviderRefreshChange, ProviderRefreshFailure};
use super::rest::config::ConfigResponse;
use super::rest::prompt::{PromptCompletedReason, PromptStatus};
use super::session::{Session, SessionLastTurnReason, SessionPendingInteraction};
use super::time::IsoDateTime;
use super::validation::{optional_non_null, required_nullable};
use super::workspace::Workspace;

macro_rules! event_type {
    ($name:ident, $wire:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name;

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str($wire)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                if value == $wire {
                    Ok(Self)
                } else {
                    Err(serde::de::Error::custom(concat!("type must be ", $wire)))
                }
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_other: f64,
    pub output: f64,
    pub input_cache_read: f64,
    pub input_cache_creation: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Completed,
    ToolCalls,
    Truncated,
    Filtered,
    Paused,
    Other,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatus {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub by_model: Option<IndexMap<String, TokenUsage>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub current_turn: Option<TokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub total: Option<TokenUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Yolo,
    Auto,
}

// Original: packages/protocol/src/events.ts, SkillSource.
// This module is expanded as the event schema migration proceeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Project,
    User,
    Extra,
    Builtin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillActivationTrigger {
    UserSlash,
    ModelTool,
    NestedSkill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShellCommandPhase {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycleStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

// Original: events.ts, PromptOrigin and its discriminated schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum PromptOrigin {
    User,
    SkillActivation {
        activation_id: String,
        skill_name: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        skill_args: Option<String>,
        trigger: SkillActivationTrigger,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        skill_type: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        skill_path: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        skill_source: Option<SkillSource>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skills: Vec<PromptSkillActivation>,
    },
    PluginCommand {
        activation_id: String,
        plugin_id: String,
        command_name: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        command_args: Option<String>,
        trigger: PluginCommandTrigger,
    },
    Injection {
        variant: String,
    },
    ShellCommand {
        phase: ShellCommandPhase,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        is_error: Option<bool>,
    },
    CompactionSummary,
    SystemTrigger {
        name: String,
    },
    Task {
        task_id: String,
        status: TaskLifecycleStatus,
        notification_id: String,
    },
    BackgroundTask {
        task_id: String,
        status: TaskLifecycleStatus,
        notification_id: String,
    },
    CronJob {
        job_id: String,
        cron: String,
        recurring: bool,
        coalesced_count: f64,
        stale: bool,
    },
    CronMissed {
        count: f64,
    },
    HookResult {
        event: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        blocked: Option<bool>,
    },
    Retry {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        trigger: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginCommandTrigger {
    #[serde(rename = "user-slash")]
    UserSlash,
}

event_type!(UserPromptOriginKind, "user");
event_type!(SkillActivationOriginKind, "skill_activation");
event_type!(PluginCommandOriginKind, "plugin_command");
event_type!(InjectionOriginKind, "injection");
event_type!(ShellCommandOriginKind, "shell_command");
event_type!(CompactionSummaryOriginKind, "compaction_summary");
event_type!(SystemTriggerOriginKind, "system_trigger");
event_type!(TaskOriginKind, "task");
event_type!(BackgroundTaskOriginKind, "background_task");
event_type!(CronMissedOriginKind, "cron_missed");
event_type!(HookResultOriginKind, "hook_result");
event_type!(RetryOriginKind, "retry");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPromptOrigin {
    pub kind: UserPromptOriginKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillActivationOrigin {
    pub kind: SkillActivationOriginKind,
    pub activation_id: String,
    pub skill_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_args: Option<String>,
    pub trigger: SkillActivationTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_source: Option<SkillSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSkillActivation {
    pub activation_id: String,
    pub skill_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_args: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_type: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_source: Option<SkillSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandOrigin {
    pub kind: PluginCommandOriginKind,
    pub activation_id: String,
    pub plugin_id: String,
    pub command_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub command_args: Option<String>,
    pub trigger: PluginCommandTrigger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionOrigin {
    pub kind: InjectionOriginKind,
    pub variant: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandOrigin {
    pub kind: ShellCommandOriginKind,
    pub phase: ShellCommandPhase,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub is_error: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSummaryOrigin {
    pub kind: CompactionSummaryOriginKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemTriggerOrigin {
    pub kind: SystemTriggerOriginKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOrigin {
    pub kind: TaskOriginKind,
    pub task_id: String,
    pub status: TaskLifecycleStatus,
    pub notification_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskOrigin {
    pub kind: BackgroundTaskOriginKind,
    pub task_id: String,
    pub status: TaskLifecycleStatus,
    pub notification_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronMissedOrigin {
    pub kind: CronMissedOriginKind,
    pub count: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookResultOrigin {
    pub kind: HookResultOriginKind,
    pub event: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub blocked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryOrigin {
    pub kind: RetryOriginKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub trigger: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalActor {
    User,
    Model,
    Runtime,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wall_clock_budget_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetReport {
    #[serde(deserialize_with = "required_nullable")]
    pub token_budget: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub turn_budget: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub wall_clock_budget_ms: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_tokens: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_turns: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_wall_clock_ms: Option<f64>,
    pub token_budget_reached: bool,
    pub turn_budget_reached: bool,
    pub wall_clock_budget_reached: bool,
    pub over_budget: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSnapshot {
    pub goal_id: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
    pub budget: GoalBudgetReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalToolResult {
    #[serde(deserialize_with = "required_nullable")]
    pub goal: Option<GoalSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalChangeStats {
    pub turns_used: f64,
    pub tokens_used: f64,
    pub wall_clock_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalChangeKind {
    Lifecycle,
    Completion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalChange {
    pub kind: GoalChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<GoalStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<GoalChangeStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<GoalActor>,
}

const KIMI_ERROR_CODES: &[&str] = &[
    "config.invalid",
    "session.not_found",
    "session.already_exists",
    "session.id_invalid",
    "session.id_required",
    "session.id_empty",
    "session.title_empty",
    "session.state_not_found",
    "session.state_invalid",
    "session.fork_active_turn",
    "session.undo_unavailable",
    "session.export_not_found",
    "session.export_missing_version",
    "session.export_output_conflict",
    "session.export_too_large",
    "session.closed",
    "session.permission_mode_invalid",
    "session.thinking_empty",
    "session.model_empty",
    "session.plan_mode_invalid",
    "session.approval_handler_error",
    "session.question_handler_error",
    "session.init_failed",
    "agent.not_found",
    "turn.agent_busy",
    "goal.already_exists",
    "goal.not_found",
    "goal.objective_empty",
    "goal.objective_too_long",
    "goal.status_invalid",
    "goal.metadata_reserved",
    "goal.not_resumable",
    "goal.unsupported_agent",
    "model.not_configured",
    "model.config_invalid",
    "profile.thinking_alias_conflict",
    "profile.unknown",
    "profile.already_bound",
    "profile.not_bound",
    "model.not_found",
    "auth.login_required",
    "auth.provisioning_required",
    "auth.token_missing",
    "auth.token_unauthorized",
    "auth.model_not_resolved",
    "context.overflow",
    "loop.max_steps_exceeded",
    "provider.api_error",
    "provider.filtered",
    "provider.rate_limit",
    "provider.auth_error",
    "provider.connection_error",
    "provider.overloaded",
    "provider.not_found",
    "skill.not_found",
    "skill.type_unsupported",
    "skill.name_empty",
    "records.write_failed",
    "compaction.failed",
    "compaction.unable",
    "task.task_id_empty",
    "usage.turn_id_conflict",
    "mcp.server_not_found",
    "mcp.server_disabled",
    "mcp.startup_failed",
    "mcp.tool_name_collision",
    "message.not_found",
    "plugin.not_found",
    "plugin.load_failed",
    "request.invalid",
    "request.work_dir_required",
    "request.prompt_input_empty",
    "prompt.not_found",
    "prompt.already_completed",
    "session.busy",
    "shell.git_bash_not_found",
    "workspace.not_found",
    "terminal.not_found",
    "file.not_found",
    "file.too_large",
    "fs.path_not_found",
    "fs.permission_denied",
    "fs.path_escapes",
    "fs.is_directory",
    "fs.is_binary",
    "fs.too_large",
    "fs.already_exists",
    "fs.too_many_results",
    "fs.grep_timeout",
    "fs.git_unavailable",
    "validation.failed",
    "not_implemented",
    "internal",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KimiErrorCode(String);

impl KimiErrorCode {
    pub fn parse(value: impl Into<String>) -> Result<Self, KimiErrorCodeError> {
        let value = value.into();
        if KIMI_ERROR_CODES.contains(&value.as_str()) {
            Ok(Self(value))
        } else {
            Err(KimiErrorCodeError(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for KimiErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for KimiErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KimiErrorCodeError(String);

impl fmt::Display for KimiErrorCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown Kimi error code: {}", self.0)
    }
}

impl std::error::Error for KimiErrorCodeError {}

// Original: events.ts, kimiErrorPayloadSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KimiErrorPayload {
    pub code: KimiErrorCode,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub details: Option<IndexMap<String, Value>>,
    pub retryable: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub cause: Option<Box<KimiErrorPayload>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfoBase {
    pub task_id: String,
    pub description: String,
    pub status: TaskLifecycleStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub detached: Option<bool>,
    pub started_at: f64,
    #[serde(deserialize_with = "required_nullable")]
    pub ended_at: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub stop_reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub terminal_notification_suppressed: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub timeout_ms: Option<f64>,
}

event_type!(ProcessTaskInfoKind, "process");
event_type!(AgentTaskInfoKind, "agent");
event_type!(QuestionTaskInfoKind, "question");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTaskInfo {
    pub kind: ProcessTaskInfoKind,
    #[serde(flatten)]
    pub base: TaskInfoBase,
    pub command: String,
    pub pid: f64,
    #[serde(deserialize_with = "required_nullable")]
    pub exit_code: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTaskInfo {
    pub kind: AgentTaskInfoKind,
    #[serde(flatten)]
    pub base: TaskInfoBase,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub subagent_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionTaskInfo {
    pub kind: QuestionTaskInfoKind,
    #[serde(flatten)]
    pub base: TaskInfoBase,
    pub question_count: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub tool_call_id: Option<String>,
}

// Original: events.ts, taskInfoSchema discriminated union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TaskInfo {
    Process {
        #[serde(flatten)]
        base: TaskInfoBase,
        command: String,
        pid: f64,
        #[serde(rename = "exitCode", deserialize_with = "required_nullable")]
        exit_code: Option<f64>,
    },
    Agent {
        #[serde(flatten)]
        base: TaskInfoBase,
        #[serde(
            rename = "agentId",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        agent_id: Option<String>,
        #[serde(
            rename = "subagentType",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        subagent_type: Option<String>,
    },
    Question {
        #[serde(flatten)]
        base: TaskInfoBase,
        #[serde(rename = "questionCount")]
        question_count: f64,
        #[serde(
            rename = "toolCallId",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        tool_call_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub compacted_count: f64,
    pub tokens_before: f64,
    pub tokens_after: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub kept_user_message_count: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub kept_head_user_message_count: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub dropped_count: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolUpdateKind {
    Stdout,
    Stderr,
    Progress,
    Status,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdate {
    pub kind: ToolUpdateKind,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub text: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub percent: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub custom_kind: Option<String>,
    #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
    pub custom_data: OptionalJsonValue,
}

pub const MCP_OAUTH_AUTHORIZATION_URL_TOOL_UPDATE: &str = "mcp.oauth.authorization_url";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthAuthorizationUrlUpdateData {
    pub server_name: String,
    pub authorization_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStreamKind {
    Assistant,
    Thinking,
    ToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentInterruptedReason {
    Aborted,
    MaxSteps,
    Error,
}

// Original: events.ts, agentPhaseSchema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentPhase {
    Idle,
    Running {
        turn_id: f64,
        step: f64,
        step_id: String,
        since: f64,
    },
    Streaming {
        turn_id: f64,
        step: f64,
        step_id: String,
        stream: AgentStreamKind,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        tool_call_id: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        tool_name: Option<String>,
        since: f64,
    },
    ToolCall {
        turn_id: f64,
        step: f64,
        tool_call_id: String,
        name: String,
        since: f64,
    },
    Retrying {
        turn_id: f64,
        step: f64,
        step_id: String,
        failed_attempt: f64,
        next_attempt: f64,
        max_attempts: f64,
        delay_ms: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        error_name: Option<String>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        status_code: Option<f64>,
        since: f64,
    },
    AwaitingApproval {
        turn_id: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "OptionalJsonValue::is_absent")]
        approval: OptionalJsonValue,
        since: f64,
    },
    Interrupted {
        turn_id: f64,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        step: Option<f64>,
        reason: AgentInterruptedReason,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        message: Option<String>,
        at: f64,
    },
    Ended {
        turn_id: f64,
        reason: TurnEndReason,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "optional_non_null"
        )]
        duration_ms: Option<f64>,
        at: f64,
    },
}

event_type!(AgentStatusUpdatedEventType, "agent.status.updated");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: AgentStatusUpdatedEventType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub thinking_effort: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub context_tokens: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub max_context_tokens: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub context_usage: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub plan_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub swarm_mode: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub permission: Option<PermissionMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub usage: Option<UsageStatus>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub phase: Option<AgentPhase>,
}

event_type!(SessionMetaUpdatedEventType, "session.meta.updated");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetaUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: SessionMetaUpdatedEventType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub patch: Option<IndexMap<String, Value>>,
}

event_type!(SessionCreatedEventType, "event.session.created");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCreatedEvent {
    #[serde(rename = "type")]
    pub event_type: SessionCreatedEventType,
    pub session: Session,
}

event_type!(WorkspaceCreatedEventType, "event.workspace.created");
event_type!(WorkspaceUpdatedEventType, "event.workspace.updated");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCreatedEvent {
    #[serde(rename = "type")]
    pub event_type: WorkspaceCreatedEventType,
    pub workspace: Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: WorkspaceUpdatedEventType,
    pub workspace: Workspace,
}

event_type!(WorkspaceDeletedEventType, "event.workspace.deleted");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeletedEvent {
    #[serde(rename = "type")]
    pub event_type: WorkspaceDeletedEventType,
    #[serde(deserialize_with = "super::validation::non_empty")]
    pub workspace_id: String,
    #[serde(deserialize_with = "super::validation::non_empty")]
    pub root: String,
}

event_type!(SessionWorkChangedEventType, "event.session.work_changed");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionWorkChangedEvent {
    #[serde(rename = "type")]
    pub event_type: SessionWorkChangedEventType,
    pub busy: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub main_turn_active: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub pending_interaction: Option<SessionPendingInteraction>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub last_turn_reason: Option<SessionLastTurnReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySessionStatus {
    Idle,
    Running,
    AwaitingApproval,
    AwaitingQuestion,
    Aborted,
}

event_type!(
    SessionStatusChangedEventType,
    "event.session.status_changed"
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusChangedEvent {
    #[serde(rename = "type")]
    pub event_type: SessionStatusChangedEventType,
    pub status: LegacySessionStatus,
    pub previous_status: LegacySessionStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty_string"
    )]
    pub current_prompt_id: Option<String>,
}

fn optional_non_empty_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    super::validation::non_empty(deserializer).map(Some)
}

event_type!(ConfigChangedEventType, "event.config.changed");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigChangedEvent {
    #[serde(rename = "type")]
    pub event_type: ConfigChangedEventType,
    pub changed_fields: Vec<String>,
    pub config: ConfigResponse,
}

event_type!(ModelCatalogChangedEventType, "event.model_catalog.changed");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCatalogChangedEvent {
    #[serde(rename = "type")]
    pub event_type: ModelCatalogChangedEventType,
    pub changed: Vec<ProviderRefreshChange>,
    #[serde(deserialize_with = "non_empty_strings")]
    pub unchanged: Vec<String>,
    pub failed: Vec<ProviderRefreshFailure>,
}

fn non_empty_strings<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<String>::deserialize(deserializer)?;
    if values.iter().any(String::is_empty) {
        Err(serde::de::Error::custom("items must not be empty"))
    } else {
        Ok(values)
    }
}

event_type!(GoalUpdatedEventType, "goal.updated");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: GoalUpdatedEventType,
    #[serde(deserialize_with = "required_nullable")]
    pub snapshot: Option<GoalSnapshot>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub change: Option<GoalChange>,
}

event_type!(SkillActivatedEventType, "skill.activated");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillActivatedEvent {
    #[serde(rename = "type")]
    pub event_type: SkillActivatedEventType,
    pub activation_id: String,
    pub skill_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_args: Option<String>,
    pub trigger: SkillActivationTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub skill_source: Option<SkillSource>,
}

event_type!(PluginCommandActivatedEventType, "plugin_command.activated");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandActivatedEvent {
    #[serde(rename = "type")]
    pub event_type: PluginCommandActivatedEventType,
    pub activation_id: String,
    pub plugin_id: String,
    pub command_name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub command_args: Option<String>,
    pub trigger: PluginCommandTrigger,
}

event_type!(ErrorEventType, "error");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEvent {
    #[serde(rename = "type")]
    pub event_type: ErrorEventType,
    #[serde(flatten)]
    pub error: KimiErrorPayload,
}

event_type!(WarningEventType, "warning");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarningEvent {
    #[serde(rename = "type")]
    pub event_type: WarningEventType,
    pub message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub code: Option<String>,
}

event_type!(TurnStartedEventType, "turn.started");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveUserMessage {
    pub prompt_id: String,
    pub user_message_id: String,
    pub created_at: IsoDateTime,
    pub content: Vec<MessageContent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedEvent {
    #[serde(rename = "type")]
    pub event_type: TurnStartedEventType,
    pub turn_id: f64,
    pub origin: PromptOrigin,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub prompt: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub user_message: Option<LiveUserMessage>,
}

event_type!(TurnEndedEventType, "turn.ended");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndedEvent {
    #[serde(rename = "type")]
    pub event_type: TurnEndedEventType,
    pub turn_id: f64,
    pub reason: TurnEndReason,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub error: Option<KimiErrorPayload>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub duration_ms: Option<f64>,
}

event_type!(TurnStepStartedEventType, "turn.step.started");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepStartedEvent {
    #[serde(rename = "type")]
    pub event_type: TurnStepStartedEventType,
    pub turn_id: f64,
    pub step: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub step_id: Option<String>,
}

event_type!(TurnStepCompletedEventType, "turn.step.completed");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepCompletedEvent {
    #[serde(rename = "type")]
    pub event_type: TurnStepCompletedEventType,
    pub turn_id: f64,
    pub step: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub step_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub usage: Option<TokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub finish_reason: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_first_token_latency_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_stream_duration_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_request_build_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_server_first_token_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_server_decode_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub llm_client_consume_ms: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub provider_finish_reason: Option<FinishReason>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub raw_finish_reason: Option<String>,
}

event_type!(TurnStepRetryingEventType, "turn.step.retrying");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepRetryingEvent {
    #[serde(rename = "type")]
    pub event_type: TurnStepRetryingEventType,
    pub turn_id: f64,
    pub step: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub step_id: Option<String>,
    pub failed_attempt: f64,
    pub next_attempt: f64,
    pub max_attempts: f64,
    pub delay_ms: f64,
    pub error_name: String,
    pub error_message: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub status_code: Option<f64>,
}

event_type!(TurnStepInterruptedEventType, "turn.step.interrupted");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStepInterruptedEvent {
    #[serde(rename = "type")]
    pub event_type: TurnStepInterruptedEventType,
    pub turn_id: f64,
    pub step: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub step_id: Option<String>,
    pub reason: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub message: Option<String>,
}

event_type!(AssistantDeltaEventType, "assistant.delta");
event_type!(ThinkingDeltaEventType, "thinking.delta");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantDeltaEvent {
    #[serde(rename = "type")]
    pub event_type: AssistantDeltaEventType,
    pub turn_id: f64,
    pub delta: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingDeltaEvent {
    #[serde(rename = "type")]
    pub event_type: ThinkingDeltaEventType,
    pub turn_id: f64,
    pub delta: String,
}

event_type!(HookResultEventType, "hook.result");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResultEvent {
    #[serde(rename = "type")]
    pub event_type: HookResultEventType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub turn_id: Option<f64>,
    pub hook_event: String,
    pub content: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub blocked: Option<bool>,
}

event_type!(ToolCallDeltaEventType, "tool.call.delta");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDeltaEvent {
    #[serde(rename = "type")]
    pub event_type: ToolCallDeltaEventType,
    pub turn_id: f64,
    pub tool_call_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub arguments_part: Option<String>,
}

event_type!(ToolCallStartedEventType, "tool.call.started");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallStartedEvent {
    #[serde(rename = "type")]
    pub event_type: ToolCallStartedEventType,
    pub turn_id: f64,
    pub tool_call_id: String,
    pub name: String,
    pub args: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub display: Option<ToolInputDisplay>,
}

event_type!(ToolProgressEventType, "tool.progress");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProgressEvent {
    #[serde(rename = "type")]
    pub event_type: ToolProgressEventType,
    pub turn_id: f64,
    pub tool_call_id: String,
    pub update: ToolUpdate,
}

event_type!(ShellOutputEventType, "shell.output");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutputEvent {
    #[serde(rename = "type")]
    pub event_type: ShellOutputEventType,
    pub command_id: String,
    pub update: ToolUpdate,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub task_id: Option<String>,
}

event_type!(ShellStartedEventType, "shell.started");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellStartedEvent {
    #[serde(rename = "type")]
    pub event_type: ShellStartedEventType,
    pub command_id: String,
    pub task_id: String,
}

event_type!(ShellCompletedEventType, "shell.completed");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCompletedEvent {
    #[serde(rename = "type")]
    pub event_type: ShellCompletedEventType,
    pub command_id: String,
    pub is_error: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub task_id: Option<String>,
}

event_type!(ToolResultEventType, "tool.result");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    #[serde(rename = "type")]
    pub event_type: ToolResultEventType,
    pub turn_id: f64,
    pub tool_call_id: String,
    pub output: Value,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub is_error: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub synthetic: Option<bool>,
}

event_type!(SubagentSpawnedEventType, "subagent.spawned");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSpawnedEvent {
    #[serde(rename = "type")]
    pub event_type: SubagentSpawnedEventType,
    pub subagent_id: String,
    pub subagent_name: String,
    pub parent_tool_call_id: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub parent_tool_call_uuid: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub parent_agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub caller_agent_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub swarm_index: Option<f64>,
    pub run_in_background: bool,
}

event_type!(SubagentStartedEventType, "subagent.started");
event_type!(SubagentSuspendedEventType, "subagent.suspended");
event_type!(SubagentCompletedEventType, "subagent.completed");
event_type!(SubagentFailedEventType, "subagent.failed");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentStartedEvent {
    #[serde(rename = "type")]
    pub event_type: SubagentStartedEventType,
    pub subagent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentSuspendedEvent {
    #[serde(rename = "type")]
    pub event_type: SubagentSuspendedEventType,
    pub subagent_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentCompletedEvent {
    #[serde(rename = "type")]
    pub event_type: SubagentCompletedEventType,
    pub subagent_id: String,
    pub result_summary: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub usage: Option<TokenUsage>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub context_tokens: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentFailedEvent {
    #[serde(rename = "type")]
    pub event_type: SubagentFailedEventType,
    pub subagent_id: String,
    pub error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

event_type!(CompactionStartedEventType, "compaction.started");
event_type!(CompactionBlockedEventType, "compaction.blocked");
event_type!(CompactionCancelledEventType, "compaction.cancelled");
event_type!(CompactionCompletedEventType, "compaction.completed");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionStartedEvent {
    #[serde(rename = "type")]
    pub event_type: CompactionStartedEventType,
    pub trigger: CompactionTrigger,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub instruction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionBlockedEvent {
    #[serde(rename = "type")]
    pub event_type: CompactionBlockedEventType,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub turn_id: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionCancelledEvent {
    #[serde(rename = "type")]
    pub event_type: CompactionCancelledEventType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionCompletedEvent {
    #[serde(rename = "type")]
    pub event_type: CompactionCompletedEventType,
    pub result: CompactionResult,
}

event_type!(TaskStartedEventType, "task.started");
event_type!(TaskTerminatedEventType, "task.terminated");
event_type!(BackgroundTaskStartedEventType, "background.task.started");
event_type!(
    BackgroundTaskTerminatedEventType,
    "background.task.terminated"
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStartedEvent {
    #[serde(rename = "type")]
    pub event_type: TaskStartedEventType,
    pub info: TaskInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskTerminatedEvent {
    #[serde(rename = "type")]
    pub event_type: TaskTerminatedEventType,
    pub info: TaskInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundTaskStartedEvent {
    #[serde(rename = "type")]
    pub event_type: BackgroundTaskStartedEventType,
    pub info: TaskInfo,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundTaskTerminatedEvent {
    #[serde(rename = "type")]
    pub event_type: BackgroundTaskTerminatedEventType,
    pub info: TaskInfo,
}

event_type!(CronJobOriginKind, "cron_job");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJobOrigin {
    pub kind: CronJobOriginKind,
    pub job_id: String,
    pub cron: String,
    pub recurring: bool,
    pub coalesced_count: f64,
    pub stale: bool,
}

event_type!(CronFiredEventType, "cron.fired");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronFiredEvent {
    #[serde(rename = "type")]
    pub event_type: CronFiredEventType,
    pub origin: CronJobOrigin,
    pub prompt: String,
}

event_type!(PromptSubmittedEventType, "prompt.submitted");
event_type!(PromptCompletedEventType, "prompt.completed");
event_type!(PromptAbortedEventType, "prompt.aborted");
event_type!(PromptSteeredEventType, "prompt.steered");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSubmittedEvent {
    #[serde(rename = "type")]
    pub event_type: PromptSubmittedEventType,
    pub prompt_id: String,
    pub user_message_id: String,
    pub status: PromptStatus,
    pub content: Vec<MessageContent>,
    pub created_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCompletedEvent {
    #[serde(rename = "type")]
    pub event_type: PromptCompletedEventType,
    pub prompt_id: String,
    pub finished_at: IsoDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub reason: Option<PromptCompletedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptAbortedEvent {
    #[serde(rename = "type")]
    pub event_type: PromptAbortedEventType,
    pub prompt_id: String,
    pub aborted_at: IsoDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSteeredEvent {
    #[serde(rename = "type")]
    pub event_type: PromptSteeredEventType,
    pub active_prompt_id: String,
    pub prompt_ids: Vec<String>,
    pub content: Vec<MessageContent>,
    /// Messages are ordered like `prompt_ids` and let clients materialize the
    /// user bubbles without racing a separate `prompt.submitted` projection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_messages: Vec<LiveUserMessage>,
    pub steered_at: IsoDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolListUpdatedReason {
    #[serde(rename = "mcp.connected")]
    McpConnected,
    #[serde(rename = "mcp.disconnected")]
    McpDisconnected,
    #[serde(rename = "mcp.failed")]
    McpFailed,
}

event_type!(ToolListUpdatedEventType, "tool.list.updated");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolListUpdatedEvent {
    #[serde(rename = "type")]
    pub event_type: ToolListUpdatedEventType,
    pub reason: ToolListUpdatedReason,
    pub server_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServerConnectionStatus {
    Pending,
    Connected,
    Failed,
    Disabled,
    NeedsAuth,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusPayload {
    pub name: String,
    pub transport: McpTransport,
    pub status: McpServerConnectionStatus,
    pub tool_count: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_null"
    )]
    pub error: Option<String>,
}

event_type!(McpServerStatusEventType, "mcp.server.status");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerStatusEvent {
    #[serde(rename = "type")]
    pub event_type: McpServerStatusEventType,
    pub server: McpServerStatusPayload,
}

// Original: events.ts, agentEventSchema.
// ConfigChangedEvent is intentionally not a variant: the TypeScript interface
// union includes it, but the authoritative runtime Zod union does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentEvent {
    Error(ErrorEvent),
    Warning(WarningEvent),
    AgentStatusUpdated(AgentStatusUpdatedEvent),
    SessionMetaUpdated(SessionMetaUpdatedEvent),
    SessionCreated(SessionCreatedEvent),
    WorkspaceCreated(WorkspaceCreatedEvent),
    WorkspaceUpdated(WorkspaceUpdatedEvent),
    WorkspaceDeleted(WorkspaceDeletedEvent),
    SessionWorkChanged(SessionWorkChangedEvent),
    SessionStatusChanged(SessionStatusChangedEvent),
    ModelCatalogChanged(ModelCatalogChangedEvent),
    GoalUpdated(GoalUpdatedEvent),
    SkillActivated(SkillActivatedEvent),
    PluginCommandActivated(PluginCommandActivatedEvent),
    TurnStarted(TurnStartedEvent),
    TurnEnded(TurnEndedEvent),
    TurnStepStarted(TurnStepStartedEvent),
    TurnStepCompleted(TurnStepCompletedEvent),
    TurnStepRetrying(TurnStepRetryingEvent),
    TurnStepInterrupted(TurnStepInterruptedEvent),
    AssistantDelta(AssistantDeltaEvent),
    HookResult(HookResultEvent),
    ThinkingDelta(ThinkingDeltaEvent),
    ToolCallDelta(ToolCallDeltaEvent),
    ToolCallStarted(ToolCallStartedEvent),
    ToolProgress(ToolProgressEvent),
    ShellOutput(ShellOutputEvent),
    ShellStarted(ShellStartedEvent),
    ShellCompleted(ShellCompletedEvent),
    ToolResult(ToolResultEvent),
    ToolListUpdated(ToolListUpdatedEvent),
    McpServerStatus(McpServerStatusEvent),
    SubagentSpawned(SubagentSpawnedEvent),
    SubagentStarted(SubagentStartedEvent),
    SubagentSuspended(SubagentSuspendedEvent),
    SubagentCompleted(SubagentCompletedEvent),
    SubagentFailed(SubagentFailedEvent),
    CompactionStarted(CompactionStartedEvent),
    CompactionBlocked(CompactionBlockedEvent),
    CompactionCancelled(CompactionCancelledEvent),
    CompactionCompleted(CompactionCompletedEvent),
    TaskStarted(TaskStartedEvent),
    TaskTerminated(TaskTerminatedEvent),
    BackgroundTaskStarted(BackgroundTaskStartedEvent),
    BackgroundTaskTerminated(BackgroundTaskTerminatedEvent),
    CronFired(CronFiredEvent),
    PromptSubmitted(PromptSubmittedEvent),
    PromptCompleted(PromptCompletedEvent),
    PromptAborted(PromptAbortedEvent),
    PromptSteered(PromptSteeredEvent),
}

// Original: events.ts, eventSchema intersection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(flatten)]
    pub event: AgentEvent,
    pub agent_id: String,
    pub session_id: String,
}

pub const VOLATILE_EVENT_TYPES: [&str; 8] = [
    "assistant.delta",
    "thinking.delta",
    "tool.call.delta",
    "tool.progress",
    "shell.output",
    "shell.started",
    "shell.completed",
    "agent.status.updated",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VolatileEventType {
    #[serde(rename = "assistant.delta")]
    AssistantDelta,
    #[serde(rename = "thinking.delta")]
    ThinkingDelta,
    #[serde(rename = "tool.call.delta")]
    ToolCallDelta,
    #[serde(rename = "tool.progress")]
    ToolProgress,
    #[serde(rename = "shell.output")]
    ShellOutput,
    #[serde(rename = "shell.started")]
    ShellStarted,
    #[serde(rename = "shell.completed")]
    ShellCompleted,
    #[serde(rename = "agent.status.updated")]
    AgentStatusUpdated,
}

// Original: events.ts, isVolatileEventType().
pub fn is_volatile_event_type(event_type: &str) -> bool {
    VOLATILE_EVENT_TYPES.contains(&event_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_snapshot_preserves_camel_case_and_required_nullable_budget() {
        let snapshot: GoalSnapshot = serde_json::from_value(serde_json::json!({
            "goalId":"g","objective":"ship","status":"active","turnsUsed":1,
            "tokensUsed":2,"wallClockMs":3,"budget":{"tokenBudget":null,
                "turnBudget":null,"wallClockBudgetMs":null,"remainingTokens":null,
                "remainingTurns":null,"remainingWallClockMs":null,
                "tokenBudgetReached":false,"turnBudgetReached":false,
                "wallClockBudgetReached":false,"overBudget":false}
        }))
        .unwrap();
        assert_eq!(snapshot.status, GoalStatus::Active);
        assert_eq!(serde_json::to_value(snapshot).unwrap()["goalId"], "g");
        assert!(serde_json::from_value::<GoalToolResult>(serde_json::json!({})).is_err());

        let origin: PromptOrigin = serde_json::from_value(serde_json::json!({
            "kind": "skill_activation",
            "activationId": "activation-1",
            "skillName": "review",
            "trigger": "user-slash",
            "skillSource": "project"
        }))
        .unwrap();
        assert!(matches!(
            origin,
            PromptOrigin::SkillActivation {
                skill_source: Some(SkillSource::Project),
                ..
            }
        ));
        let origin: PromptOrigin = serde_json::from_value(serde_json::json!({
            "kind": "skill_activation",
            "activationId": "activation-1",
            "skillName": "pdf",
            "trigger": "user-slash",
            "skills": [
                {"activationId": "activation-1", "skillName": "pdf"},
                {"activationId": "activation-2", "skillName": "docs"}
            ]
        }))
        .unwrap();
        assert!(matches!(
            origin,
            PromptOrigin::SkillActivation { skills, .. } if skills.len() == 2
        ));
        assert!(
            serde_json::from_value::<PromptOrigin>(serde_json::json!({
                "kind": "retry", "trigger": null
            }))
            .is_err()
        );

        let task: TaskInfo = serde_json::from_value(serde_json::json!({
            "kind": "process", "taskId": "bash-1", "description": "sleep",
            "status": "running", "startedAt": 1, "endedAt": null,
            "command": "sleep 1", "pid": 123, "exitCode": null
        }))
        .unwrap();
        assert!(matches!(
            task,
            TaskInfo::Process {
                exit_code: None,
                ..
            }
        ));
        assert!(
            serde_json::from_value::<ProcessTaskInfo>(serde_json::json!({
                "kind": "process", "taskId": "bash-1", "description": "sleep",
                "status": "running", "startedAt": 1, "endedAt": null,
                "command": "sleep 1", "pid": 123, "exitCode": null
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<KimiErrorPayload>(serde_json::json!({
                "code": "unknown.code", "message": "bad", "retryable": false
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AgentPhase>(serde_json::json!({
                "kind": "streaming", "turnId": 1, "step": 2, "stepId": "s",
                "stream": "tool_call", "since": 3
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<WorkspaceDeletedEvent>(serde_json::json!({
                "type": "event.workspace.deleted", "workspace_id": "", "root": "/repo"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GoalUpdatedEvent>(serde_json::json!({
                "type": "goal.updated", "snapshot": null
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<WarningEvent>(serde_json::json!({
                "type": "error", "message": "wrong literal"
            }))
            .is_err()
        );
        let tool: ToolCallStartedEvent = serde_json::from_value(serde_json::json!({
            "type": "tool.call.started", "turnId": 1, "toolCallId": "call-1",
            "name": "bash", "args": {"command": "pwd"},
            "display": {"kind": "command", "command": "pwd", "language": "bash"}
        }))
        .unwrap();
        assert_eq!(tool.name, "bash");
        assert!(
            serde_json::from_value::<ShellCompletedEvent>(serde_json::json!({
                "type": "shell.started", "commandId": "cmd", "isError": false
            }))
            .is_err()
        );
        let prompt: PromptSubmittedEvent = serde_json::from_value(serde_json::json!({
            "type": "prompt.submitted", "promptId": "p", "userMessageId": "m",
            "status": "blocked", "content": [{"type": "text", "text": "hello"}],
            "createdAt": "2026-06-11T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(prompt.created_at, "2026-06-11T00:00:00.000Z");
        let started: TurnStartedEvent = serde_json::from_value(serde_json::json!({
            "type": "turn.started", "turnId": 7, "origin": {"kind": "user"},
            "prompt": "hello",
            "userMessage": {
                "promptId": "p", "userMessageId": "m",
                "createdAt": "2026-06-11T00:00:00Z",
                "content": [{"type": "text", "text": "hello"}]
            }
        }))
        .unwrap();
        assert_eq!(started.user_message.unwrap().user_message_id, "m");
        assert!(
            serde_json::from_value::<McpServerStatusPayload>(serde_json::json!({
                "name": "server", "transport": "sse", "status": "connected", "toolCount": 2
            }))
            .is_err()
        );
        let event: Event = serde_json::from_value(serde_json::json!({
            "type": "assistant.delta", "agentId": "main", "sessionId": "sess-1",
            "turnId": 1, "delta": "hello"
        }))
        .unwrap();
        assert!(matches!(event.event, AgentEvent::AssistantDelta(_)));
        assert!(is_volatile_event_type("assistant.delta"));
        assert!(!is_volatile_event_type("turn.ended"));
        assert!(
            serde_json::from_value::<AgentEvent>(serde_json::json!({
                "type": "unknown.event"
            }))
            .is_err()
        );
    }
}
