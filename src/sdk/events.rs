use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::types::{
    BackgroundTaskInfo, GoalChange, GoalSnapshot, McpServerStatusSnapshot, PermissionMode,
    PromptOrigin, SessionUsage, SkillSource, TokenUsage, ToolUpdate,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Completed,
    ToolCalls,
    Truncated,
    Filtered,
    Paused,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnEndReason {
    Completed,
    Cancelled,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActivationTrigger {
    #[serde(rename = "user-slash")]
    UserSlash,
    #[serde(rename = "model-tool")]
    ModelTool,
    #[serde(rename = "nested-skill")]
    NestedSkill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactionTrigger {
    Manual,
    Auto,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronJobOriginKind {
    #[serde(rename = "cron_job")]
    CronJob,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KimiErrorPayload {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Box<KimiErrorPayload>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionResult {
    pub summary: String,
    pub compacted_count: u64,
    pub tokens_before: u64,
    pub tokens_after: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_user_message_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_head_user_message_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJobOrigin {
    pub kind: CronJobOriginKind,
    pub job_id: String,
    pub cron: String,
    pub recurring: bool,
    pub coalesced_count: u64,
    pub stale: bool,
}

/// Event types consumed by the app-level TUI. Other protocol events are kept
/// as `Unknown`, matching the original controllers' default no-op branch.
///
/// Original:
///   packages/protocol/src/events.ts
///   AgentEvent
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum AgentEvent {
    #[serde(rename = "error")]
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Map<String, Value>>,
        retryable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<Box<KimiErrorPayload>>,
    },
    #[serde(rename = "warning")]
    Warning {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    #[serde(rename = "agent.status.updated")]
    AgentStatusUpdated {
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking_effort: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_context_tokens: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_usage: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        plan_mode: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        swarm_mode: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        permission: Option<PermissionMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<SessionUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<Value>,
    },
    #[serde(rename = "session.meta.updated")]
    SessionMetaUpdated {
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        patch: Option<Map<String, Value>>,
    },
    #[serde(rename = "goal.updated")]
    GoalUpdated {
        snapshot: Option<GoalSnapshot>,
        #[serde(skip_serializing_if = "Option::is_none")]
        change: Option<GoalChange>,
    },
    #[serde(rename = "skill.activated")]
    SkillActivated {
        activation_id: String,
        skill_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_args: Option<String>,
        trigger: ActivationTrigger,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skill_source: Option<SkillSource>,
    },
    #[serde(rename = "plugin_command.activated")]
    PluginCommandActivated {
        activation_id: String,
        plugin_id: String,
        command_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        command_args: Option<String>,
        trigger: ActivationTrigger,
    },
    #[serde(rename = "turn.started")]
    TurnStarted {
        turn_id: u64,
        origin: PromptOrigin,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    #[serde(rename = "turn.ended")]
    TurnEnded {
        turn_id: u64,
        reason: TurnEndReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<KimiErrorPayload>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
    },
    #[serde(rename = "turn.step.started")]
    TurnStepStarted {
        turn_id: u64,
        step: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
    },
    #[serde(rename = "turn.step.completed")]
    TurnStepCompleted {
        turn_id: u64,
        step: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        finish_reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_first_token_latency_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_stream_duration_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_request_build_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_server_first_token_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_server_decode_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        llm_client_consume_ms: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_finish_reason: Option<FinishReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_finish_reason: Option<String>,
    },
    #[serde(rename = "turn.step.retrying")]
    TurnStepRetrying {
        turn_id: u64,
        step: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
        failed_attempt: u64,
        next_attempt: u64,
        max_attempts: u64,
        delay_ms: u64,
        error_name: String,
        error_message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status_code: Option<u16>,
    },
    #[serde(rename = "turn.step.interrupted")]
    TurnStepInterrupted {
        turn_id: u64,
        step: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    #[serde(rename = "assistant.delta")]
    AssistantDelta { turn_id: u64, delta: String },
    #[serde(rename = "hook.result")]
    HookResult {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
        hook_event: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        blocked: Option<bool>,
    },
    #[serde(rename = "thinking.delta")]
    ThinkingDelta { turn_id: u64, delta: String },
    #[serde(rename = "tool.call.delta")]
    ToolCallDelta {
        turn_id: u64,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments_part: Option<String>,
    },
    #[serde(rename = "tool.call.started")]
    ToolCallStarted {
        turn_id: u64,
        tool_call_id: String,
        name: String,
        args: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<Value>,
    },
    #[serde(rename = "tool.progress")]
    ToolProgress {
        turn_id: u64,
        tool_call_id: String,
        update: ToolUpdate,
    },
    #[serde(rename = "shell.output")]
    ShellOutput {
        command_id: String,
        update: ToolUpdate,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    #[serde(rename = "shell.started")]
    ShellStarted { command_id: String, task_id: String },
    #[serde(rename = "shell.completed")]
    ShellCompleted {
        command_id: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<String>,
    },
    #[serde(rename = "tool.result")]
    ToolResult {
        turn_id: u64,
        tool_call_id: String,
        output: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        synthetic: Option<bool>,
    },
    #[serde(rename = "tool.list.updated")]
    ToolListUpdated {
        reason: ToolListUpdatedReason,
        server_name: String,
    },
    #[serde(rename = "mcp.server.status")]
    McpServerStatus { server: McpServerStatusSnapshot },
    #[serde(rename = "subagent.spawned")]
    SubagentSpawned {
        subagent_id: String,
        subagent_name: String,
        parent_tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_tool_call_uuid: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_agent_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        caller_agent_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        swarm_index: Option<u64>,
        run_in_background: bool,
    },
    #[serde(rename = "subagent.started")]
    SubagentStarted { subagent_id: String },
    #[serde(rename = "subagent.suspended")]
    SubagentSuspended { subagent_id: String, reason: String },
    #[serde(rename = "subagent.completed")]
    SubagentCompleted {
        subagent_id: String,
        result_summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_tokens: Option<u64>,
    },
    #[serde(rename = "subagent.failed")]
    SubagentFailed { subagent_id: String, error: String },
    #[serde(rename = "compaction.started")]
    CompactionStarted {
        trigger: CompactionTrigger,
        #[serde(skip_serializing_if = "Option::is_none")]
        instruction: Option<String>,
    },
    #[serde(rename = "compaction.blocked")]
    CompactionBlocked {
        #[serde(skip_serializing_if = "Option::is_none")]
        turn_id: Option<u64>,
    },
    #[serde(rename = "compaction.cancelled")]
    CompactionCancelled,
    #[serde(rename = "compaction.completed")]
    CompactionCompleted { result: CompactionResult },
    #[serde(rename = "task.started")]
    TaskStarted { info: BackgroundTaskInfo },
    #[serde(rename = "task.terminated")]
    TaskTerminated { info: BackgroundTaskInfo },
    #[serde(rename = "background.task.started")]
    BackgroundTaskStarted { info: BackgroundTaskInfo },
    #[serde(rename = "background.task.terminated")]
    BackgroundTaskTerminated { info: BackgroundTaskInfo },
    #[serde(rename = "cron.fired")]
    CronFired {
        origin: CronJobOrigin,
        prompt: String,
    },
    #[serde(other)]
    Unknown,
}

/// Session event envelope emitted by the SDK.
///
/// Original: `packages/protocol/src/events.ts`, `Event`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub agent_id: String,
    pub session_id: String,
    #[serde(flatten)]
    pub event: AgentEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_streaming_tool_and_turn_wire_events() {
        let delta: Event = serde_json::from_value(serde_json::json!({
            "type": "tool.call.delta",
            "agentId": "agent-1",
            "sessionId": "session-1",
            "turnId": 4,
            "toolCallId": "call-1",
            "name": "Read",
            "argumentsPart": "{\"path\":"
        }))
        .expect("tool delta event");
        assert!(matches!(
            delta.event,
            AgentEvent::ToolCallDelta { turn_id: 4, ref tool_call_id, .. }
                if tool_call_id == "call-1"
        ));

        let ended: Event = serde_json::from_value(serde_json::json!({
            "type": "turn.ended",
            "agentId": "agent-1",
            "sessionId": "session-1",
            "turnId": 4,
            "reason": "failed",
            "error": {
                "code": "provider.filtered",
                "message": "filtered",
                "retryable": false
            }
        }))
        .expect("turn ended event");
        assert!(matches!(
            ended.event,
            AgentEvent::TurnEnded {
                reason: TurnEndReason::Failed,
                error: Some(KimiErrorPayload { ref code, .. }),
                ..
            } if code == "provider.filtered"
        ));
    }

    #[test]
    fn decodes_subagent_compaction_and_task_wire_events() {
        let spawned: Event = serde_json::from_value(serde_json::json!({
            "type": "subagent.spawned",
            "agentId": "main",
            "sessionId": "session-1",
            "subagentId": "sub-1",
            "subagentName": "explorer",
            "parentToolCallId": "call-1",
            "runInBackground": true
        }))
        .expect("subagent event");
        assert!(matches!(
            spawned.event,
            AgentEvent::SubagentSpawned {
                run_in_background: true,
                ..
            }
        ));

        let compacted: Event = serde_json::from_value(serde_json::json!({
            "type": "compaction.completed",
            "agentId": "main",
            "sessionId": "session-1",
            "result": {
                "summary": "summary",
                "compactedCount": 3,
                "tokensBefore": 100,
                "tokensAfter": 20
            }
        }))
        .expect("compaction event");
        assert!(matches!(
            compacted.event,
            AgentEvent::CompactionCompleted {
                result: CompactionResult {
                    compacted_count: 3,
                    ..
                }
            }
        ));

        let task: Event = serde_json::from_value(serde_json::json!({
            "type": "task.started",
            "agentId": "main",
            "sessionId": "session-1",
            "info": {
                "taskId": "task-1",
                "description": "build",
                "status": "running",
                "startedAt": 1,
                "endedAt": null,
                "kind": "process",
                "command": "cargo test",
                "pid": 42,
                "exitCode": null
            }
        }))
        .expect("task event");
        assert!(matches!(task.event, AgentEvent::TaskStarted { .. }));
    }

    #[test]
    fn unknown_protocol_events_follow_original_no_op_default_branch() {
        let event: Event = serde_json::from_value(serde_json::json!({
            "type": "prompt.completed",
            "agentId": "main",
            "sessionId": "session-1",
            "promptId": "prompt-1",
            "finishedAt": "2026-01-01T00:00:00Z"
        }))
        .expect("unknown app event remains decodable");
        assert_eq!(event.event, AgentEvent::Unknown);
    }
}
