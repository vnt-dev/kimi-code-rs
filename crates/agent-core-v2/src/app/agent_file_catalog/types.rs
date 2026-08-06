//! Shared agent-file discovery models.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/types.ts`.

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AgentFileSource {
    Project,
    User,
    Extra,
    Explicit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFileRoot {
    pub path: String,
    pub source: AgentFileSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFileDefinition {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub is_override: bool,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub subagents: Option<Vec<String>>,
    pub model: Option<String>,
    pub prompt: String,
    pub path: String,
    pub source: AgentFileSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkippedAgentFile {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentFileDiscoveryResult {
    pub agents: Vec<AgentFileDefinition>,
    pub skipped: Vec<SkippedAgentFile>,
    pub scanned_roots: Vec<String>,
}
