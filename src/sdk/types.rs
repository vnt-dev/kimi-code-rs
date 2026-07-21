use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Result emitted after a lifecycle hook runs.
///
/// Original:
///   packages/kimi-code-sdk/src/types/events.ts
///   HookResultEvent
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookResultEvent {
    pub hook_event: String,
    pub content: String,
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginSource {
    LocalPath,
    ZipUrl,
    Github,
}

impl PluginSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local-path",
            Self::ZipUrl => "zip-url",
            Self::Github => "github",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    Ok,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginGithubRefKind {
    Branch,
    Tag,
    Sha,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginGithubRef {
    pub kind: PluginGithubRefKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginGithubMetadata {
    pub owner: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: PluginGithubRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_sha: Option<String>,
}

/// Plugin metadata projected by the SDK for list views.
///
/// Original:
///   packages/agent-core-v2/src/app/plugin/types.ts
///   PluginSummary
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub enabled: bool,
    pub state: PluginState,
    pub skill_count: usize,
    pub mcp_server_count: usize,
    pub enabled_mcp_server_count: usize,
    pub hook_count: usize,
    pub command_count: usize,
    pub has_errors: bool,
    pub source: PluginSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<PluginGithubMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginDiagnosticSeverity {
    Error,
    Warn,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    pub severity: PluginDiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PluginAuthor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginSessionStart {
    pub skill: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInterface {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_name: Option<String>,
    #[serde(rename = "websiteURL", skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommandEntry {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start: Option<PluginSessionStart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<PluginCommandEntry>>,
    #[serde(rename = "interface", skip_serializing_if = "Option::is_none")]
    pub interface_config: Option<PluginInterface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_instructions: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginManifestKind {
    KimiPluginRoot,
    KimiPluginDir,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpServerInfo {
    pub name: String,
    pub runtime_name: String,
    pub enabled: bool,
    pub transport: McpServerTransport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_keys: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header_keys: Option<Vec<String>>,
}

/// Original:
///   packages/agent-core-v2/src/app/plugin/types.ts
///   PluginInfo
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub enabled: bool,
    pub state: PluginState,
    pub skill_count: usize,
    pub mcp_server_count: usize,
    pub enabled_mcp_server_count: usize,
    pub hook_count: usize,
    pub command_count: usize,
    pub has_errors: bool,
    pub source: PluginSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<PluginGithubMetadata>,
    pub root: String,
    pub installed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_kind: Option<PluginManifestKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    pub mcp_servers: Vec<PluginMcpServerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_manifest_path: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// Provider-defined thinking effort. Known values include `off` and `on`, but
/// providers may expose arbitrary named effort levels.
///
/// Original:
///   packages/kosong/src/provider.ts
///   ThinkingEffort
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThinkingEffort(String);

impl ThinkingEffort {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ThinkingEffort {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

/// Lifecycle state of a background task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Killed,
    Lost,
}

/// Kind-specific background-task fields, serialized with the original `kind`
/// discriminator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BackgroundTaskKind {
    Process {
        command: String,
        pid: u32,
        #[serde(rename = "exitCode")]
        exit_code: Option<i32>,
    },
    Agent {
        #[serde(rename = "agentId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_id: Option<String>,
        #[serde(rename = "subagentType")]
        #[serde(skip_serializing_if = "Option::is_none")]
        subagent_type: Option<String>,
    },
    Question {
        #[serde(rename = "questionCount")]
        question_count: usize,
        #[serde(rename = "toolCallId")]
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
    },
}

/// Snapshot of a process, subagent, or question background task.
///
/// Original:
///   packages/agent-core/src/agent/background/task.ts
///   BackgroundTaskInfo
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskInfo {
    pub task_id: String,
    pub description: String,
    pub status: BackgroundTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_notification_suppressed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<f64>,
    #[serde(flatten)]
    pub kind: BackgroundTaskKind,
}

/// Summary returned by the session-listing SDK surface.
///
/// Original:
///   packages/node-sdk/src/types.ts
///   SessionSummary
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<String>,
    pub work_dir: String,
    pub session_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_dirs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    Complete,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalBudgetReport {
    pub token_budget: Option<u64>,
    pub turn_budget: Option<u64>,
    pub wall_clock_budget_ms: Option<u64>,
    pub remaining_tokens: Option<u64>,
    pub remaining_turns: Option<u64>,
    pub remaining_wall_clock_ms: Option<u64>,
    pub token_budget_reached: bool,
    pub turn_budget_reached: bool,
    pub wall_clock_budget_reached: bool,
    pub over_budget: bool,
}

/// Public computed view of the current goal.
///
/// Original:
///   packages/protocol/src/events.ts
///   GoalSnapshot
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalSnapshot {
    pub goal_id: String,
    pub objective: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_criterion: Option<String>,
    pub status: GoalStatus,
    pub turns_used: u64,
    pub tokens_used: u64,
    pub wall_clock_ms: u64,
    pub budget: GoalBudgetReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Manual,
    Yolo,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEnvironment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term_program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub term_program_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multiplexer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    Project,
    User,
    Extra,
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SkillSource>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub skill_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_model_invocation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_sub_skill: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input: u64,
    pub input_cache_read: u64,
    pub input_cache_creation: u64,
    pub input_other: u64,
    pub output: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_model: Option<BTreeMap<String, TokenUsage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_turn: Option<TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<TokenUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub thinking_effort: String,
    pub permission: PermissionMode,
    pub plan_mode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swarm_mode: Option<bool>,
    pub context_tokens: u64,
    pub max_context_tokens: u64,
    pub context_usage: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<SessionUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptPart {
    Text {
        text: String,
    },
    ImageUrl {
        #[serde(rename = "imageUrl")]
        image_url: MediaUrl,
    },
    VideoUrl {
        #[serde(rename = "videoUrl")]
        video_url: MediaUrl,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Think {
        think: String,
    },
    ImageUrl {
        #[serde(rename = "imageUrl")]
        image_url: MediaUrl,
    },
    AudioUrl {
        #[serde(rename = "audioUrl")]
        audio_url: MediaUrl,
    },
    VideoUrl {
        #[serde(rename = "videoUrl")]
        video_url: MediaUrl,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    #[serde(rename = "type", default = "function_tool_call_type")]
    pub tool_type: String,
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

fn function_tool_call_type() -> String {
    "function".to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptOriginKind {
    Injection,
    SystemTrigger,
    CompactionSummary,
    HookResult,
    CronJob,
    CronMissed,
    User,
    BackgroundTask,
    SkillActivation,
    PluginCommand,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptOrigin {
    pub kind: PromptOriginKind,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextMessage {
    pub role: ContextMessageRole,
    pub content: Vec<ContentPart>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<PromptOrigin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandDef {
    pub plugin_id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerTransport {
    #[serde(rename = "stdio")]
    Stdio,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "sse")]
    Sse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpServerStatus {
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "connected")]
    Connected,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "needs-auth")]
    NeedsAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatusSnapshot {
    pub name: String,
    pub transport: McpServerTransport,
    pub status: McpServerStatus,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronTaskSnapshot {
    pub id: String,
    pub cron: String,
    pub recurring: bool,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<u64>,
    pub next_fire_at: Option<u64>,
}

#[cfg(test)]
mod plugin_info_tests {
    use super::*;

    #[test]
    fn preserves_plugin_info_wire_names_and_nested_manifest_data() {
        let value = serde_json::json!({
            "id": "demo",
            "displayName": "Demo",
            "enabled": true,
            "state": "ok",
            "skillCount": 1,
            "mcpServerCount": 1,
            "enabledMcpServerCount": 1,
            "hookCount": 0,
            "commandCount": 0,
            "hasErrors": false,
            "source": "zip-url",
            "root": "/plugins/demo",
            "installedAt": "2026-01-01T00:00:00Z",
            "manifestKind": "kimi-plugin-root",
            "manifest": {
                "name": "demo",
                "interface": {
                    "displayName": "Demo",
                    "websiteURL": "https://example.com"
                },
                "mcpServers": { "server": { "type": "http" } },
                "hooks": [{ "event": "stop" }],
                "commands": [{ "path": "deploy.md", "name": "deploy" }]
            },
            "mcpServers": [{
                "name": "server",
                "runtimeName": "plugin:demo:server",
                "enabled": true,
                "transport": "http",
                "url": "https://example.com/mcp"
            }],
            "diagnostics": []
        });
        let info = serde_json::from_value::<PluginInfo>(value).ok();
        assert!(info.is_some());
        let encoded = info.and_then(|value| serde_json::to_value(value).ok());
        assert_eq!(
            encoded
                .as_ref()
                .and_then(|value| value.pointer("/manifest/interface/websiteURL")),
            Some(&Value::String("https://example.com".to_owned()))
        );
        assert_eq!(
            encoded.as_ref().and_then(|value| value.get("manifestKind")),
            Some(&Value::String("kimi-plugin-root".to_owned()))
        );
    }
}
