use serde::{Deserialize, Serialize};

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
