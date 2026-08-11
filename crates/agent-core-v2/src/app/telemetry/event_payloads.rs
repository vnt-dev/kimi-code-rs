//! Typed telemetry event payloads and emission adapter.
//!
//! Original: payload interfaces and `track2()` typing in
//! `packages/agent-core-v2/src/app/telemetry/events.ts`.

use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use super::{
    agent_telemetry_context::AgentTelemetryMode,
    contract::{TelemetryProperties, TelemetryServiceContract},
    events::TelemetryEventContext,
};

#[derive(Debug, thiserror::Error)]
pub enum TelemetryPayloadError {
    #[error("failed to serialize telemetry payload: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("telemetry event payload must serialize as an object")]
    NotObject,
}

pub trait TelemetryEventPayload: Serialize {
    const NAME: &'static str;
    const CONTEXT: TelemetryEventContext;

    fn to_telemetry_properties(&self) -> Result<TelemetryProperties, TelemetryPayloadError> {
        let Value::Object(object) = serde_json::to_value(self)? else {
            return Err(TelemetryPayloadError::NotObject);
        };
        Ok(object
            .into_iter()
            .map(|(key, value)| (key, Some(value)))
            .collect())
    }
}

pub trait TelemetryServiceEventExt: TelemetryServiceContract {
    // Original: ITelemetryService.track2().
    fn track_event<E: TelemetryEventPayload>(
        &self,
        payload: &E,
    ) -> Result<(), TelemetryPayloadError> {
        let properties = payload.to_telemetry_properties()?;
        self.track(E::NAME, Some(&properties));
        Ok(())
    }
}

impl<T: TelemetryServiceContract + ?Sized> TelemetryServiceEventExt for T {}

macro_rules! impl_event_payload {
    ($payload:ty, $name:literal, $context:ident) => {
        impl TelemetryEventPayload for $payload {
            const NAME: &'static str = $name;
            const CONTEXT: TelemetryEventContext = TelemetryEventContext::$context;
        }
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnInterruptReason {
    UserCancelled,
    Aborted,
    MaxSteps,
    Error,
    Filtered,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallOutcome {
    Success,
    Error,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallDupType {
    Normal,
    SameStep,
    CrossStep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallErrorType {
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillTrigger {
    UserSlash,
    ModelTool,
    NestedSkill,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelSource {
    Streaming,
    Compacting,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryPermissionMode {
    Manual,
    Yolo,
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Approve,
    Deny,
    Ask,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionApprovalResult {
    Error,
    ApprovedForSession,
    Approved,
    Rejected,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanResolutionOutcome {
    Approved,
    Dismissed,
    RejectedAndExited,
    Revise,
    Rejected,
    AutoApproved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanEnterOutcome {
    AutoApproved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionSource {
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskCreatedKind {
    Bash,
    Agent,
    Question,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskCompletedKind {
    Agent,
    Process,
    Question,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionAnswerMethod {
    Enter,
    Space,
    NumberKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryGoalActor {
    User,
    Model,
    Runtime,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallRepeatAction {
    None,
    R1,
    R2,
    R3,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum RgBinarySource {
    #[serde(rename = "share-bin-cached")]
    ShareBinCached,
    #[serde(rename = "vendor")]
    Vendor,
    #[serde(rename = "share-bin-downloaded")]
    ShareBinDownloaded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RgFallbackOutcome {
    Resolved,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsGrepFallbackReason {
    RgMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageCompressOutcome {
    Compressed,
    PassthroughFast,
    PassthroughGuard,
    PassthroughUnsupported,
    PassthroughUnhelpful,
    PassthroughError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageCropErrorKind {
    Empty,
    UnsupportedFormat,
    RegionInvalid,
    TooLarge,
    OutOfBounds,
    Budget,
    DecodeFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoUploadOutcome {
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TurnStartedEvent {
    pub turn_id: crate::agent::TurnId,
    pub mode: AgentTelemetryMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,
}
impl_event_payload!(TurnStartedEvent, "turn_started", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TurnInterruptedEvent {
    pub turn_id: crate::agent::TurnId,
    pub at_step: u64,
    pub mode: AgentTelemetryMode,
    pub interrupt_reason: TurnInterruptReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(TurnInterruptedEvent, "turn_interrupted", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TurnEndedEvent {
    pub turn_id: crate::agent::TurnId,
    pub reason: TurnEndReason,
    pub duration_ms: u64,
    pub mode: AgentTelemetryMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(TurnEndedEvent, "turn_ended", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCallEvent {
    pub turn_id: crate::agent::TurnId,
    pub tool_call_id: String,
    pub tool_name: String,
    pub outcome: ToolCallOutcome,
    pub duration_ms: u64,
    pub dup_type: ToolCallDupType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<ToolCallErrorType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(ToolCallEvent, "tool_call", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ApiErrorEvent {
    pub error_type: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub retryable: bool,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_no: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(ApiErrorEvent, "api_error", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkillInvokedEvent {
    pub skill_name: String,
    pub trigger: SkillTrigger,
}
impl_event_payload!(SkillInvokedEvent, "skill_invoked", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FlowInvokedEvent {
    pub flow_name: String,
}
impl_event_payload!(FlowInvokedEvent, "flow_invoked", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InputSteerEvent {
    pub parts: u64,
}
impl_event_payload!(InputSteerEvent, "input_steer", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CancelEvent {
    pub from: CancelSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(CancelEvent, "cancel", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConversationUndoEvent {
    pub count: u64,
}
impl_event_payload!(ConversationUndoEvent, "conversation_undo", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct YoloToggleEvent {
    pub enabled: bool,
}
impl_event_payload!(YoloToggleEvent, "yolo_toggle", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AfkToggleEvent {
    pub enabled: bool,
}
impl_event_payload!(AfkToggleEvent, "afk_toggle", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PermissionPolicyDecisionEvent {
    pub turn_id: crate::agent::TurnId,
    pub tool_call_id: String,
    pub policy_name: String,
    pub tool_name: String,
    pub permission_mode: TelemetryPermissionMode,
    pub decision: PermissionDecision,
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}
impl_event_payload!(
    PermissionPolicyDecisionEvent,
    "permission_policy_decision",
    Agent
);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PermissionApprovalResultEvent {
    pub turn_id: crate::agent::TurnId,
    pub tool_call_id: String,
    pub policy_name: Option<String>,
    pub tool_name: String,
    pub permission_mode: TelemetryPermissionMode,
    pub result: PermissionApprovalResult,
    pub approval_surface: String,
    pub duration_ms: u64,
    pub session_cache_written: bool,
    pub has_feedback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(
    PermissionApprovalResultEvent,
    "permission_approval_result",
    Agent
);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanSubmittedEvent {
    pub has_options: bool,
}
impl_event_payload!(PlanSubmittedEvent, "plan_submitted", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanResolvedEvent {
    pub outcome: PlanResolutionOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chosen_option: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_feedback: Option<bool>,
}
impl_event_payload!(PlanResolvedEvent, "plan_resolved", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanEnterResolvedEvent {
    pub outcome: PlanEnterOutcome,
}
impl_event_payload!(PlanEnterResolvedEvent, "plan_enter_resolved", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompactionFinishedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    pub source: CompactionSource,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub duration_ms: u64,
    pub compacted_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_count: Option<u64>,
    pub retry_count: u64,
    pub round: u64,
    pub thinking_effort: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cache_read: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_cache_creation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(CompactionFinishedEvent, "compaction_finished", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CompactionFailedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    pub source: CompactionSource,
    pub tokens_before: u64,
    pub duration_ms: u64,
    pub round: u64,
    pub retry_count: u64,
    pub thinking_effort: String,
    pub error_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(CompactionFailedEvent, "compaction_failed", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContextProjectionRepairedEvent {
    pub reordered: u64,
    pub synthesized: u64,
    pub dropped_orphan: u64,
    pub duplicate_calls_dropped: u64,
    pub duplicate_results_dropped: u64,
    pub leading_dropped: u64,
    pub assistants_merged: u64,
    pub whitespace_dropped: u64,
    pub vacuous_dropped: u64,
}
impl_event_payload!(
    ContextProjectionRepairedEvent,
    "context_projection_repaired",
    Agent
);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BackgroundTaskCreatedEvent {
    pub task_id: String,
    pub kind: BackgroundTaskCreatedKind,
}
impl_event_payload!(BackgroundTaskCreatedEvent, "background_task_created", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BackgroundTaskCompletedEvent {
    pub task_id: String,
    pub kind: BackgroundTaskCompletedKind,
    pub duration_ms: Option<u64>,
    pub status: BackgroundTaskStatus,
}
impl_event_payload!(
    BackgroundTaskCompletedEvent,
    "background_task_completed",
    Agent
);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelSwitchEvent {
    pub model: String,
}
impl_event_payload!(ModelSwitchEvent, "model_switch", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ThinkingToggleEvent {
    pub enabled: bool,
    pub effort: String,
    pub from: String,
}
impl_event_payload!(ThinkingToggleEvent, "thinking_toggle", Agent);

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct QuestionDismissedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(QuestionDismissedEvent, "question_dismissed", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuestionAnsweredEvent {
    pub answered: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<QuestionAnswerMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(QuestionAnsweredEvent, "question_answered", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalBudgetProperties {
    pub has_token_budget: bool,
    pub has_turn_budget: bool,
    pub has_wall_clock_budget: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalCreatedEvent {
    pub actor: TelemetryGoalActor,
    pub replace: bool,
}
impl_event_payload!(GoalCreatedEvent, "goal_created", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalBudgetSetEvent {
    pub actor: TelemetryGoalActor,
    #[serde(flatten)]
    pub budget: GoalBudgetProperties,
}
impl_event_payload!(GoalBudgetSetEvent, "goal_budget_set", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalContinuedEvent {
    pub turns_used: u64,
}
impl_event_payload!(GoalContinuedEvent, "goal_continued", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalClearedEvent {
    pub actor: TelemetryGoalActor,
}
impl_event_payload!(GoalClearedEvent, "goal_cleared", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalStatusChangedEvent {
    pub actor: TelemetryGoalActor,
    pub status: GoalStatus,
    pub turns_used: u64,
    pub tokens_used: u64,
    pub wall_clock_ms: u64,
    #[serde(flatten)]
    pub budget: GoalBudgetProperties,
}
impl_event_payload!(GoalStatusChangedEvent, "goal_status_changed", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCallDedupDetectedEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    pub step_no: u64,
    pub tool_call_id: String,
    pub tool_name: String,
    pub dup_type: ToolCallDupType,
    pub args_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(
    ToolCallDedupDetectedEvent,
    "tool_call_dedup_detected",
    Agent
);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolCallRepeatEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<crate::agent::TurnId>,
    pub tool_name: String,
    pub repeat_count: u64,
    pub action: ToolCallRepeatAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}
impl_event_payload!(ToolCallRepeatEvent, "tool_call_repeat", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GrepToolRgFallbackEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<RgBinarySource>,
    pub outcome: RgFallbackOutcome,
}
impl_event_payload!(GrepToolRgFallbackEvent, "grep_tool_rg_fallback", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GlobToolRgFallbackEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<RgBinarySource>,
    pub outcome: RgFallbackOutcome,
}
impl_event_payload!(GlobToolRgFallbackEvent, "glob_tool_rg_fallback", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FsGrepNodeFallbackEvent {
    pub reason: FsGrepFallbackReason,
}
impl_event_payload!(FsGrepNodeFallbackEvent, "fs_grep_node_fallback", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubagentCreatedEvent {
    pub subagent_name: String,
    pub run_in_background: bool,
    pub agent_id: String,
    pub parent_agent_id: String,
    pub parent_tool_call_id: String,
}
impl_event_payload!(SubagentCreatedEvent, "subagent_created", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct McpConnectedEvent {
    pub server_count: u64,
    pub total_count: u64,
}
impl_event_payload!(McpConnectedEvent, "mcp_connected", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct McpFailedEvent {
    pub failed_count: u64,
    pub total_count: u64,
}
impl_event_payload!(McpFailedEvent, "mcp_failed", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CronMissedEvent {
    pub count: u64,
}
impl_event_payload!(CronMissedEvent, "cron_missed", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CronScheduledEvent {
    pub recurring: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}
impl_event_payload!(CronScheduledEvent, "cron_scheduled", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CronDeletedEvent {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}
impl_event_payload!(CronDeletedEvent, "cron_deleted", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CronFiredEvent {
    pub recurring: bool,
    pub coalesced_count: u64,
    pub stale: bool,
    pub buffered: bool,
}
impl_event_payload!(CronFiredEvent, "cron_fired", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageCompressEvent {
    pub source: String,
    pub outcome: ImageCompressOutcome,
    pub input_mime: String,
    pub output_mime: String,
    pub original_bytes: u64,
    pub final_bytes: u64,
    pub original_width: u64,
    pub original_height: u64,
    pub final_width: u64,
    pub final_height: u64,
    pub exif_transposed: bool,
    pub duration_ms: u64,
}
impl_event_payload!(ImageCompressEvent, "image_compress", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageCropEvent {
    pub source: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ImageCropErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region_area_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_bytes: Option<u64>,
    pub duration_ms: u64,
}
impl_event_payload!(ImageCropEvent, "image_crop", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VideoUploadEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    pub mime_type: String,
    pub size_bytes: u64,
    pub outcome: VideoUploadOutcome,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
}
impl_event_payload!(VideoUploadEvent, "video_upload", Agent);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionStartedEvent {
    pub resumed: bool,
}
impl_event_payload!(SessionStartedEvent, "session_started", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionLoadFailedEvent {
    pub reason: String,
}
impl_event_payload!(SessionLoadFailedEvent, "session_load_failed", None);

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FirstLaunchEvent {}
impl_event_payload!(FirstLaunchEvent, "first_launch", None);

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExitEvent {
    pub duration_ms: u64,
}
impl_event_payload!(ExitEvent, "exit", None);

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use crate::app::telemetry::{TelemetryAppender, TelemetryService, TelemetryServiceContract};

    use super::*;

    #[derive(Default)]
    struct Capture(Mutex<Vec<(String, TelemetryProperties)>>);

    #[async_trait]
    impl TelemetryAppender for Capture {
        fn track(&self, event: &str, properties: Option<&TelemetryProperties>) {
            self.0
                .lock()
                .unwrap()
                .push((event.into(), properties.cloned().unwrap_or_default()));
        }
    }

    #[test]
    fn typed_emission_preserves_names_enums_optional_fields_and_nulls() {
        let service = TelemetryService::new();
        let capture = Arc::new(Capture::default());
        let erased: Arc<dyn TelemetryAppender> = capture.clone();
        service.set_appender(erased);

        service
            .track_event(&TurnStartedEvent {
                turn_id: crate::agent::TurnId::new(7),
                mode: AgentTelemetryMode::Plan,
                provider_type: None,
                protocol: Some("anthropic".into()),
                thinking_effort: None,
            })
            .unwrap();
        service
            .track_event(&BackgroundTaskCompletedEvent {
                task_id: "task-1".into(),
                kind: BackgroundTaskCompletedKind::Process,
                duration_ms: None,
                status: BackgroundTaskStatus::TimedOut,
            })
            .unwrap();

        let events = capture.0.lock().unwrap();
        assert_eq!(events[0].0, "turn_started");
        assert_eq!(events[0].1["mode"], Some(Value::from("plan")));
        assert!(!events[0].1.contains_key("provider_type"));
        assert_eq!(events[0].1["protocol"], Some(Value::from("anthropic")));
        assert_eq!(events[1].1["duration_ms"], Some(Value::Null));
        assert_eq!(events[1].1["status"], Some(Value::from("timed_out")));
        assert_eq!(
            BackgroundTaskCompletedEvent::CONTEXT,
            TelemetryEventContext::Agent
        );
    }
}
