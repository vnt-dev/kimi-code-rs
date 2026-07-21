use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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
    pub step: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}
