use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

use super::display::OptionalJsonValue;
use super::validation::{optional_non_null, required_nullable};

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
    }
}
