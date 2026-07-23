//! Plugin manifest, installed-state, and public API models.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/types.ts`.

use std::{collections::HashMap, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::agent::{external_hooks::HookDefConfig, mcp::McpServerConfig};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginDiagnosticSeverity {
    Error,
    Warn,
    Info,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    pub severity: PluginDiagnosticSeverity,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginAuthor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginSessionStart {
    pub skill: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInterface {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_name: Option<String>,
    #[serde(
        rename = "websiteURL",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub website_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandEntry {
    pub path: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_start: Option<PluginSessionStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Vec<HookDefConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<PluginCommandEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<PluginInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_instructions: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginMcpServerState {
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilityState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<HashMap<String, PluginMcpServerState>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginMcpTransport {
    Stdio,
    Http,
    Sse,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpServerInfo {
    pub name: String,
    pub runtime_name: String,
    pub enabled: bool,
    pub transport: PluginMcpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_keys: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_keys: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommandDef {
    pub plugin_id: String,
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginManifestKind {
    KimiPluginRoot,
    KimiPluginDir,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginSource {
    LocalPath,
    ZipUrl,
    Github,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginState {
    Ok,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginGithubRefKind {
    Branch,
    Tag,
    Sha,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PluginGithubRef {
    pub kind: PluginGithubRefKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginGithubMetadata {
    pub owner: String,
    pub repo: String,
    #[serde(rename = "ref")]
    pub reference: PluginGithubRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub id: String,
    pub root: String,
    pub source: PluginSource,
    pub enabled: bool,
    pub state: PluginState,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<PluginCapabilityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<PluginGithubMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_instructions: Option<String>,
    pub skill_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_kind: Option<PluginManifestKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadowed_manifest_path: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<PluginGithubMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    #[serde(flatten)]
    pub summary: PluginSummary,
    pub root: String,
    pub installed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_kind: Option<PluginManifestKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<PluginManifest>,
    pub mcp_servers: Vec<PluginMcpServerInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadowed_manifest_path: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnabledPluginSessionStart {
    pub plugin_id: String,
    pub skill_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReloadError {
    pub id: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReloadSummary {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<ReloadError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginUpdateStatus {
    pub id: String,
    pub source: PluginSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<PluginGithubRef>,
    pub latest: PluginGithubRef,
    pub display_version: String,
    pub update_available: bool,
}

pub static PLUGIN_NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").expect("plugin name regex must compile")
});

pub fn normalize_plugin_id(name: &str) -> String {
    name.to_lowercase()
}

pub fn is_valid_plugin_name(name: &str) -> bool {
    PLUGIN_NAME_REGEX.is_match(name)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn identifiers_and_wire_enums_preserve_source_rules() {
        assert_eq!(normalize_plugin_id("My_Plugin"), "my_plugin");
        assert!(is_valid_plugin_name("plugin-1_name"));
        assert!(is_valid_plugin_name("a"));
        assert!(!is_valid_plugin_name("Upper"));
        assert!(!is_valid_plugin_name(&"a".repeat(65)));
        assert_eq!(
            serde_json::to_value(PluginSource::LocalPath).unwrap(),
            "local-path"
        );
        assert_eq!(
            serde_json::to_value(PluginManifestKind::KimiPluginDir).unwrap(),
            "kimi-plugin-dir"
        );
    }

    #[test]
    fn manifest_keeps_exact_camel_case_and_acronym_fields() {
        let manifest: PluginManifest = serde_json::from_value(json!({
            "name": "demo",
            "sessionStart": {"skill": "start"},
            "interface": {"displayName": "Demo", "websiteURL": "https://example.com"},
            "mcpServers": {"server": {"transport": "stdio", "command": "node"}},
            "hooks": [{"event": "Stop", "command": "cleanup"}],
            "skillInstructions": "rules"
        }))
        .unwrap();
        assert_eq!(
            manifest.interface.as_ref().unwrap().website_url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            manifest.mcp_servers.as_ref().unwrap()["server"].transport(),
            "stdio"
        );
        let serialized = serde_json::to_value(manifest).unwrap();
        assert_eq!(serialized["interface"]["websiteURL"], "https://example.com");
        assert_eq!(serialized["sessionStart"]["skill"], "start");
    }
}
