use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::sdk::types::PromptPart;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueuedMessageMode {
    Prompt,
    Bash,
}

/// Message held while the current turn or compaction is still active.
///
/// Original: `apps/kimi-code/src/tui/types.ts`, `QueuedMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedMessage {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<PromptPart>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_attachment_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<QueuedMessageMode>,
}

impl QueuedMessage {
    pub fn prompt(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            agent_id: None,
            parts: None,
            image_attachment_ids: None,
            mode: None,
        }
    }

    pub fn bash(text: impl Into<String>) -> Self {
        Self {
            mode: Some(QueuedMessageMode::Bash),
            ..Self::prompt(text)
        }
    }

    pub fn is_bash(&self) -> bool {
        self.mode == Some(QueuedMessageMode::Bash)
    }
}

/// One Ctrl-S steering unit, preserving media parts extracted at submission.
///
/// Original: `apps/kimi-code/src/tui/types.ts`, `SteerInputItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteerInputItem {
    pub text: String,
    pub parts: Option<Vec<PromptPart>>,
    pub image_attachment_ids: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BannerDisplay {
    Always,
    Once,
    Cooldown,
}

/// Optional announcement shown between the welcome panel and transcript.
///
/// Original: `apps/kimi-code/src/tui/types.ts`, `BannerState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BannerState {
    pub key: String,
    pub tag: Option<String>,
    pub main_text: String,
    pub sub_text: Option<String>,
    pub display: BannerDisplay,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_hours: Option<f64>,
}

/// Metadata recorded for a background subagent transcript entry.
///
/// Original:
///   apps/kimi-code/src/tui/types.ts
///   BackgroundAgentMetadata
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundAgentMetadata {
    pub agent_id: String,
    pub parent_tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Visual phase used by background-agent transcript cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundAgentStatusPhase {
    Started,
    Completed,
    Failed,
}

/// Renderer-independent data for a background-agent transcript card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundAgentStatusData {
    pub phase: BackgroundAgentStatusPhase,
    pub headline: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionTranscriptData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTranscriptData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coalesced_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missed_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandTranscriptData {
    pub activation_id: String,
    pub plugin_id: String,
    pub command_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub trigger: SkillActivationTrigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEntryKind {
    Welcome,
    User,
    Assistant,
    ToolCall,
    Thinking,
    Status,
    SkillActivation,
    PluginCommand,
    Cron,
    Goal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptRenderMode {
    Markdown,
    Plain,
    Notice,
}

/// Renderer-independent transcript record. Component-specific payloads will
/// be added alongside their migrated components; the windowing logic relies
/// only on the stable identity and turn association represented here.
///
/// Original:
///   apps/kimi-code/src/tui/types.ts
///   TranscriptEntry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    pub id: String,
    pub kind: TranscriptEntryKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub render_mode: TranscriptRenderMode,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_text: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bullet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_data: Option<CompactionTranscriptData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_data: Option<CronTranscriptData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_agent_status: Option<BackgroundAgentStatusData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_attachment_ids: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_activation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_args: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_trigger: Option<SkillActivationTrigger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_command_data: Option<PluginCommandTranscriptData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillActivationTrigger {
    UserSlash,
    ModelTool,
    NestedSkill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallBlockData {
    pub id: String,
    pub name: String,
    pub args: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_arguments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentReplayBlockData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentReplayToolCallData {
    pub id: String,
    pub name: String,
    pub args: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ToolResultBlockData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentReplayBlockData {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<SubagentReplayToolCallData>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultBlockData {
    pub tool_call_id: String,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
}

#[cfg(test)]
mod tool_call_replay_tests {
    use super::*;

    #[test]
    fn round_trips_subagent_replay_state_with_external_field_names() {
        let value = serde_json::json!({
            "id": "parent",
            "name": "Agent",
            "args": {"description": "inspect"},
            "subagent": {
                "id": "agent-1",
                "name": "explorer",
                "text": "done",
                "toolCalls": [{
                    "id": "child-1",
                    "name": "Read",
                    "args": {"path": "src/main.rs"},
                    "result": {
                        "tool_call_id": "child-1",
                        "output": "content",
                        "is_error": false
                    }
                }]
            }
        });
        let call: ToolCallBlockData = serde_json::from_value(value).expect("valid tool call");
        let subagent = call.subagent.as_ref().expect("subagent replay");
        assert_eq!(subagent.id, "agent-1");
        assert_eq!(subagent.tool_calls.as_ref().map(Vec::len), Some(1));

        let encoded = serde_json::to_value(call).expect("serializable tool call");
        assert_eq!(encoded["subagent"]["toolCalls"][0]["name"], "Read");
        assert!(encoded["subagent"].get("tool_calls").is_none());
    }
}
