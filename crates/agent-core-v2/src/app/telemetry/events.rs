//! Telemetry event registry and review metadata.
//!
//! Original: `packages/agent-core-v2/src/app/telemetry/events.ts`.
//!
//! Rust adaptation: this module isolates runtime registry metadata from the
//! typed emission layer so metadata consumers do not depend on payload types.

use std::sync::LazyLock;

use indexmap::IndexMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryEventContext {
    None,
    Agent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryEventMeta {
    pub owner: &'static str,
    pub comment: &'static str,
    pub properties: IndexMap<&'static str, &'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryEventDefinition {
    pub context: TelemetryEventContext,
    pub meta: TelemetryEventMeta,
}

// Original: defineTelemetryEvent().
pub fn define_telemetry_event(meta: TelemetryEventMeta) -> TelemetryEventDefinition {
    TelemetryEventDefinition {
        context: TelemetryEventContext::None,
        meta,
    }
}

// Original: defineAgentTelemetryEvent().
pub fn define_agent_telemetry_event(meta: TelemetryEventMeta) -> TelemetryEventDefinition {
    TelemetryEventDefinition {
        context: TelemetryEventContext::Agent,
        meta,
    }
}

pub static AGENT_TELEMETRY_CONTEXT_PROPERTIES: LazyLock<IndexMap<&'static str, &'static str>> =
    LazyLock::new(|| IndexMap::from([("agent_id", "Agent id (main or subagent scope id)")]));

pub static TELEMETRY_EVENT_DEFINITIONS: LazyLock<IndexMap<&'static str, TelemetryEventDefinition>> =
    LazyLock::new(|| {
        IndexMap::from([
            (
                "turn_started",
                agent(
                    "A turn starts running.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("mode", "Agent mode the turn runs in"),
                        ("provider_type", "Provider protocol type"),
                        ("protocol", "Request protocol"),
                        (
                            "thinking_effort",
                            "Effective thinking effort the turn runs with",
                        ),
                    ]),
                ),
            ),
            (
                "turn_interrupted",
                agent(
                    "A running turn is interrupted.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("at_step", "Step index the turn reached before interruption"),
                        ("mode", "Agent mode the turn ran in"),
                        ("interrupt_reason", "Why the turn was interrupted"),
                        ("provider_type", "Provider protocol type"),
                        ("protocol", "Request protocol"),
                        (
                            "thinking_effort",
                            "Effective thinking effort the turn ran with",
                        ),
                        (
                            "trace_id",
                            "Trace id of the most recent LLM request in this turn (the failed request when the turn errored); absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "turn_ended",
                agent(
                    "A turn ends, unconditionally.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("reason", "How the turn ended"),
                        ("duration_ms", "Turn wall-clock time in milliseconds"),
                        ("mode", "Agent mode the turn ran in"),
                        ("provider_type", "Provider protocol type"),
                        ("protocol", "Request protocol"),
                        (
                            "thinking_effort",
                            "Effective thinking effort the turn ran with",
                        ),
                        (
                            "trace_id",
                            "Trace id of the most recent LLM request in this turn; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "tool_call",
                agent(
                    "A tool call finishes execution.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("tool_call_id", "Provider-assigned tool call id"),
                        ("tool_name", "Registered tool name"),
                        ("outcome", "Execution outcome"),
                        ("duration_ms", "Wall-clock execution time in milliseconds"),
                        (
                            "dup_type",
                            "Whether the call was a duplicate within the same step or across steps",
                        ),
                        ("error_type", "Error category when the call failed"),
                        (
                            "trace_id",
                            "Trace id of the LLM request that produced this tool call; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "api_error",
                agent(
                    "An LLM API request fails.",
                    properties([
                        ("error_type", "Classified error category"),
                        ("model", "Model id the request targeted"),
                        ("alias", "Model alias the request targeted"),
                        ("retryable", "Whether the error is retryable"),
                        ("duration_ms", "Request wall-clock time in milliseconds"),
                        ("status_code", "HTTP status code when available"),
                        ("provider_type", "Provider protocol type"),
                        ("protocol", "Request protocol"),
                        (
                            "input_tokens",
                            "Current turn's accumulated total input tokens",
                        ),
                        (
                            "turn_id",
                            "Per-agent turn index when the request belongs to a turn; omitted for out-of-turn operations",
                        ),
                        (
                            "request_kind",
                            "Request source vocabulary: 'turn' for turn requests, the operation's requestKind (e.g. 'full_compaction') otherwise",
                        ),
                        (
                            "step_no",
                            "Step index within the turn, when the request belongs to a turn step",
                        ),
                        (
                            "trace_id",
                            "Trace id of the failed request, from its response headers or its error response; absent when the failure happened before any response headers arrived (network errors, local aborts), and for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "skill_invoked",
                agent(
                    "A skill is invoked.",
                    properties([
                        ("skill_name", "Skill name"),
                        ("trigger", "How the skill was triggered"),
                    ]),
                ),
            ),
            (
                "flow_invoked",
                agent(
                    "A flow-type skill is invoked.",
                    properties([("flow_name", "Flow name")]),
                ),
            ),
            (
                "input_steer",
                agent(
                    "The user steers input while a turn is running.",
                    properties([("parts", "Number of input parts")]),
                ),
            ),
            (
                "cancel",
                agent(
                    "The user cancels ongoing work.",
                    properties([
                        ("from", "What was running when cancelled"),
                        (
                            "trace_id",
                            "Trace id of the in-flight request, or of the most recent request between steps; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "conversation_undo",
                agent(
                    "The user undoes conversation entries.",
                    properties([("count", "Number of entries undone")]),
                ),
            ),
            (
                "yolo_toggle",
                agent(
                    "Yolo permission mode is toggled.",
                    properties([("enabled", "Whether yolo mode is now enabled")]),
                ),
            ),
            (
                "afk_toggle",
                agent(
                    "AFK (auto) permission mode is toggled.",
                    properties([("enabled", "Whether auto mode is now enabled")]),
                ),
            ),
            (
                "permission_policy_decision",
                agent(
                    "A permission policy evaluates a tool call.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("tool_call_id", "Provider-assigned tool call id"),
                        ("policy_name", "Name of the deciding policy"),
                        ("tool_name", "Tool being gated"),
                        ("permission_mode", "Active permission mode"),
                        ("decision", "Policy decision"),
                    ]),
                ),
            ),
            (
                "permission_approval_result",
                agent(
                    "A permission approval prompt resolves.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session",
                        ),
                        ("tool_call_id", "Provider-assigned tool call id"),
                        (
                            "policy_name",
                            "Name of the asking policy, null when unknown",
                        ),
                        ("tool_name", "Tool being approved"),
                        ("permission_mode", "Active permission mode"),
                        ("result", "How the approval resolved"),
                        ("approval_surface", "UI surface that presented the approval"),
                        ("duration_ms", "Time the approval took in milliseconds"),
                        (
                            "session_cache_written",
                            "Whether a session approval rule was cached",
                        ),
                        ("has_feedback", "Whether the user attached feedback"),
                        (
                            "trace_id",
                            "Trace id of the LLM request that produced the gated tool call; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "plan_submitted",
                agent(
                    "A plan is submitted for review.",
                    properties([("has_options", "Whether the plan offered selectable options")]),
                ),
            ),
            (
                "plan_resolved",
                agent(
                    "A submitted plan is resolved.",
                    properties([
                        ("outcome", "How the plan was resolved"),
                        ("chosen_option", "Label of the option the user chose"),
                        (
                            "has_feedback",
                            "Whether the user attached revision feedback",
                        ),
                    ]),
                ),
            ),
            (
                "plan_enter_resolved",
                agent(
                    "A request to enter plan mode is resolved.",
                    properties([("outcome", "How the request was resolved")]),
                ),
            ),
            (
                "compaction_finished",
                agent(
                    "Context compaction completes.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index when compaction ran inside a turn; omitted for manual compaction between turns",
                        ),
                        (
                            "source",
                            "Whether compaction was triggered manually or automatically",
                        ),
                        ("tokens_before", "Token count before compaction"),
                        ("tokens_after", "Token count after compaction"),
                        ("duration_ms", "Compaction wall-clock time in milliseconds"),
                        ("compacted_count", "Number of entries compacted"),
                        ("dropped_count", "Number of entries dropped"),
                        ("retry_count", "Number of retries attempted"),
                        ("round", "Compaction round index"),
                        ("thinking_effort", "Thinking effort level in effect"),
                        (
                            "input_tokens",
                            "Total input tokens (other + cache read + cache creation)",
                        ),
                        ("output_tokens", "Output tokens"),
                        ("input_cache_read", "Cache-read input tokens"),
                        ("input_cache_creation", "Cache-creation input tokens"),
                        (
                            "trace_id",
                            "Trace id of the final compaction request round; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "compaction_failed",
                agent(
                    "Context compaction fails.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index when compaction ran inside a turn; omitted for manual compaction between turns",
                        ),
                        (
                            "source",
                            "Whether compaction was triggered manually or automatically",
                        ),
                        ("tokens_before", "Token count before compaction"),
                        (
                            "duration_ms",
                            "Wall-clock time until failure in milliseconds",
                        ),
                        ("round", "Compaction round index"),
                        ("retry_count", "Number of retries attempted"),
                        ("thinking_effort", "Thinking effort level in effect"),
                        ("error_type", "Error class name"),
                        (
                            "trace_id",
                            "Trace id of the failed compaction request, from its response headers or its error response; absent when the failure happened before any request or before response headers arrived (network errors), and for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "context_projection_repaired",
                agent(
                    "The context projector repairs the outgoing request to keep it wire-valid.",
                    properties([
                        ("reordered", "Tool results moved back next to their call"),
                        ("synthesized", "Placeholder results invented for lost ones"),
                        ("dropped_orphan", "Results with no matching call dropped"),
                        (
                            "duplicate_calls_dropped",
                            "Tool calls with an already-seen id dropped",
                        ),
                        (
                            "duplicate_results_dropped",
                            "Second results for an already-answered id dropped",
                        ),
                        ("leading_dropped", "Leading non-user messages dropped"),
                        ("assistants_merged", "Consecutive assistant messages merged"),
                        ("whitespace_dropped", "Whitespace-only text blocks dropped"),
                        (
                            "vacuous_dropped",
                            "Messages dropped because every recorded part serialized to nothing",
                        ),
                    ]),
                ),
            ),
            (
                "background_task_created",
                agent(
                    "A background task is created.",
                    properties([
                        (
                            "task_id",
                            "Background task id; joins background_task_created with background_task_completed",
                        ),
                        (
                            "kind",
                            "Task kind; process tasks retain the legacy bash value",
                        ),
                    ]),
                ),
            ),
            (
                "background_task_completed",
                agent(
                    "A background task reaches a terminal state.",
                    properties([
                        (
                            "task_id",
                            "Background task id; joins background_task_created with background_task_completed",
                        ),
                        ("kind", "Task kind"),
                        (
                            "duration_ms",
                            "Task wall-clock time in milliseconds, null when unknown",
                        ),
                        ("status", "Terminal task status"),
                    ]),
                ),
            ),
            (
                "model_switch",
                agent(
                    "The active model is bound or switched.",
                    properties([("model", "Model alias")]),
                ),
            ),
            (
                "thinking_toggle",
                agent(
                    "Thinking effort is toggled.",
                    properties([
                        ("enabled", "Whether thinking is now enabled"),
                        ("effort", "New thinking effort level"),
                        ("from", "Previous thinking effort level"),
                    ]),
                ),
            ),
            (
                "question_dismissed",
                agent(
                    "A user question prompt is dismissed.",
                    properties([(
                        "trace_id",
                        "Trace id of the LLM request that produced the questioning tool call; absent for non-Kimi protocols",
                    )]),
                ),
            ),
            (
                "question_answered",
                agent(
                    "A user question prompt is answered.",
                    properties([
                        ("answered", "Number of questions answered"),
                        ("method", "Input method used to answer"),
                        (
                            "trace_id",
                            "Trace id of the LLM request that produced the questioning tool call; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "goal_created",
                agent(
                    "A goal is created.",
                    properties([
                        ("actor", "Who created the goal"),
                        ("replace", "Whether the goal replaces an existing one"),
                    ]),
                ),
            ),
            (
                "goal_budget_set",
                agent(
                    "A goal budget is set.",
                    properties([
                        ("actor", "Who set the budget"),
                        ("has_token_budget", "Whether a token budget was set"),
                        ("has_turn_budget", "Whether a turn budget was set"),
                        (
                            "has_wall_clock_budget",
                            "Whether a wall-clock budget was set",
                        ),
                    ]),
                ),
            ),
            (
                "goal_continued",
                agent(
                    "A goal continues into another turn.",
                    properties([("turns_used", "Turns consumed so far")]),
                ),
            ),
            (
                "goal_cleared",
                agent(
                    "A goal is cleared.",
                    properties([("actor", "Who cleared the goal")]),
                ),
            ),
            (
                "goal_status_changed",
                agent(
                    "A goal changes status.",
                    properties([
                        ("actor", "Who changed the status"),
                        ("status", "New goal status"),
                        ("turns_used", "Turns consumed so far"),
                        ("tokens_used", "Tokens consumed so far"),
                        (
                            "wall_clock_ms",
                            "Wall-clock time consumed so far in milliseconds",
                        ),
                        ("has_token_budget", "Whether a token budget was set"),
                        ("has_turn_budget", "Whether a turn budget was set"),
                        (
                            "has_wall_clock_budget",
                            "Whether a wall-clock budget was set",
                        ),
                    ]),
                ),
            ),
            (
                "tool_call_dedup_detected",
                agent(
                    "A duplicate tool call is detected.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session; omitted when no turn is active",
                        ),
                        ("step_no", "Step index within the turn"),
                        ("tool_call_id", "Provider-assigned tool call id"),
                        ("tool_name", "Registered tool name"),
                        (
                            "dup_type",
                            "Whether the duplicate is within the same step or across steps",
                        ),
                        ("args_hash", "Hash of the tool call arguments"),
                        (
                            "trace_id",
                            "Trace id of the LLM request that produced the duplicate tool call; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "tool_call_repeat",
                agent(
                    "A repeated tool call streak is detected.",
                    properties([
                        (
                            "turn_id",
                            "Per-agent turn index (main or subagent); pair with agent_id to locate a turn within a session; omitted when no turn is active",
                        ),
                        ("tool_name", "Registered tool name"),
                        ("repeat_count", "Length of the repeat streak"),
                        ("action", "Intervention action taken"),
                        (
                            "trace_id",
                            "Trace id of the LLM request that produced the repeated tool call; absent for non-Kimi protocols",
                        ),
                    ]),
                ),
            ),
            (
                "grep_tool_rg_fallback",
                agent(
                    "The grep tool falls back when resolving ripgrep.",
                    properties([
                        ("source", "Where ripgrep was resolved from"),
                        ("outcome", "Whether the fallback resolved or failed"),
                    ]),
                ),
            ),
            (
                "glob_tool_rg_fallback",
                agent(
                    "The glob tool falls back when resolving ripgrep.",
                    properties([
                        ("source", "Where ripgrep was resolved from"),
                        ("outcome", "Whether the fallback resolved or failed"),
                    ]),
                ),
            ),
            (
                "fs_grep_node_fallback",
                plain(
                    "The fs grep path falls back to the node implementation.",
                    properties([("reason", "Why the fallback was taken")]),
                ),
            ),
            (
                "subagent_created",
                plain(
                    "A subagent run is created.",
                    properties([
                        ("subagent_name", "Profile name of the subagent"),
                        (
                            "run_in_background",
                            "Whether the subagent runs in the background",
                        ),
                        ("agent_id", "Child agent id"),
                        ("parent_agent_id", "Parent (caller) agent id"),
                        (
                            "parent_tool_call_id",
                            "Tool call id of the launching call in the parent agent; '' when not launched from a tool call",
                        ),
                    ]),
                ),
            ),
            (
                "mcp_connected",
                plain(
                    "MCP servers connect at session start.",
                    properties([
                        ("server_count", "Number of servers connected"),
                        ("total_count", "Total number of configured servers"),
                    ]),
                ),
            ),
            (
                "mcp_failed",
                plain(
                    "MCP servers fail to connect at session start.",
                    properties([
                        ("failed_count", "Number of servers that failed"),
                        ("total_count", "Total number of configured servers"),
                    ]),
                ),
            ),
            (
                "cron_missed",
                plain(
                    "Cron tasks fire late after being slept through.",
                    properties([("count", "Number of tasks that missed their fire time")]),
                ),
            ),
            (
                "cron_scheduled",
                plain(
                    "A cron task is scheduled.",
                    properties([
                        ("recurring", "Whether the task repeats"),
                        (
                            "agent_id",
                            "Agent that scheduled the task; omitted for session-level scheduling",
                        ),
                    ]),
                ),
            ),
            (
                "cron_deleted",
                plain(
                    "A cron task is deleted.",
                    properties([
                        ("task_id", "Cron task id"),
                        (
                            "agent_id",
                            "Agent that deleted the task; omitted for session-level deletion (e.g. stale auto-removal)",
                        ),
                    ]),
                ),
            ),
            (
                "cron_fired",
                plain(
                    "A cron task fires.",
                    properties([
                        ("recurring", "Whether the task repeats"),
                        (
                            "coalesced_count",
                            "How many ideal fires collapsed into this delivery",
                        ),
                        (
                            "stale",
                            "Whether the task fired past its staleness threshold",
                        ),
                        (
                            "buffered",
                            "Whether the fire was buffered while a turn was running",
                        ),
                    ]),
                ),
            ),
            (
                "image_compress",
                plain(
                    "An image is compressed before being sent to the model.",
                    properties([
                        ("source", "Where the image came from"),
                        ("outcome", "Compression outcome"),
                        ("input_mime", "Input MIME type"),
                        ("output_mime", "Output MIME type"),
                        ("original_bytes", "Input size in bytes"),
                        ("final_bytes", "Output size in bytes"),
                        ("original_width", "Input width in pixels"),
                        ("original_height", "Input height in pixels"),
                        ("final_width", "Output width in pixels"),
                        ("final_height", "Output height in pixels"),
                        ("exif_transposed", "Whether EXIF orientation was applied"),
                        ("duration_ms", "Compression wall-clock time in milliseconds"),
                    ]),
                ),
            ),
            (
                "image_crop",
                plain(
                    "An image is cropped to a region before being sent to the model.",
                    properties([
                        ("source", "Where the image came from"),
                        ("ok", "Whether the crop succeeded"),
                        ("error_kind", "Failure category when the crop failed"),
                        ("resized", "Whether the crop was resized"),
                        ("original_width", "Input width in pixels"),
                        ("original_height", "Input height in pixels"),
                        (
                            "region_area_ratio",
                            "Cropped region area relative to the original",
                        ),
                        ("final_bytes", "Output size in bytes"),
                        ("duration_ms", "Crop wall-clock time in milliseconds"),
                    ]),
                ),
            ),
            (
                "video_upload",
                agent(
                    "A video is uploaded for the model.",
                    properties([
                        ("model", "Model the video is uploaded for"),
                        ("provider_type", "Provider protocol type"),
                        ("protocol", "Upload protocol"),
                        ("mime_type", "Video MIME type"),
                        ("size_bytes", "Video size in bytes"),
                        ("outcome", "Upload outcome"),
                        ("duration_ms", "Upload wall-clock time in milliseconds"),
                        ("error_type", "Error class name when the upload failed"),
                    ]),
                ),
            ),
            (
                "session_started",
                plain(
                    "A session becomes active (created, forked, or resumed).",
                    properties([("resumed", "Whether the session was resumed from disk")]),
                ),
            ),
            (
                "session_load_failed",
                plain(
                    "A session resume fails.",
                    properties([("reason", "Error code, error name, or unknown")]),
                ),
            ),
            (
                "first_launch",
                plain(
                    "The CLI runs for the first time on this device.",
                    IndexMap::new(),
                ),
            ),
            (
                "exit",
                plain(
                    "A CLI run exits.",
                    properties([("duration_ms", "Run wall-clock time in milliseconds")]),
                ),
            ),
        ])
    });

fn properties<const N: usize>(
    entries: [(&'static str, &'static str); N],
) -> IndexMap<&'static str, &'static str> {
    IndexMap::from(entries)
}

fn meta(
    comment: &'static str,
    properties: IndexMap<&'static str, &'static str>,
) -> TelemetryEventMeta {
    TelemetryEventMeta {
        owner: "kimi-code",
        comment,
        properties,
    }
}

fn agent(
    comment: &'static str,
    properties: IndexMap<&'static str, &'static str>,
) -> TelemetryEventDefinition {
    define_agent_telemetry_event(meta(comment, properties))
}

fn plain(
    comment: &'static str,
    properties: IndexMap<&'static str, &'static str>,
) -> TelemetryEventDefinition {
    define_telemetry_event(meta(comment, properties))
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;

    #[test]
    fn registry_names_and_metadata_match_review_invariants() {
        let snake_case = Regex::new(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$").unwrap();
        assert_eq!(TELEMETRY_EVENT_DEFINITIONS.len(), 50);
        for (name, definition) in TELEMETRY_EVENT_DEFINITIONS.iter() {
            assert!(snake_case.is_match(name), "event name {name}");
            assert!(!definition.meta.owner.is_empty(), "{name}: owner");
            assert!(!definition.meta.comment.is_empty(), "{name}: comment");
            for (property, comment) in &definition.meta.properties {
                assert!(snake_case.is_match(property), "{name}.{property}");
                assert!(!comment.is_empty(), "{name}.{property}: comment");
            }
            if definition.context == TelemetryEventContext::Agent {
                assert!(!definition.meta.properties.contains_key("agent_id"));
            }
        }
        assert_eq!(
            TELEMETRY_EVENT_DEFINITIONS["goal_created"].context,
            TelemetryEventContext::Agent
        );
        assert_eq!(
            TELEMETRY_EVENT_DEFINITIONS["image_compress"].context,
            TelemetryEventContext::None
        );
        assert_eq!(
            AGENT_TELEMETRY_CONTEXT_PROPERTIES.get("agent_id"),
            Some(&"Agent id (main or subagent scope id)")
        );
    }
}
