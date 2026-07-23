//! Plugin manifest discovery, validation, and path normalization.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/manifest.ts`.

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use serde_json::{Map, Value};

use crate::agent::{
    external_hooks::{HookDefConfig, parse_hook_def_config},
    mcp::{McpServerConfig, parse_mcp_server_config},
};

use super::types::{
    PluginAuthor, PluginCommandEntry, PluginDiagnostic, PluginDiagnosticSeverity, PluginInterface,
    PluginManifest, PluginManifestKind, PluginSessionStart, is_valid_plugin_name,
};

const KIMI_PLUGIN_ROOT_PATH: &str = "kimi.plugin.json";
const KIMI_PLUGIN_DIR_PATH: &str = ".kimi-plugin/plugin.json";
const UNSUPPORTED_RUNTIME_FIELDS: [&str; 6] = [
    "tools",
    "apps",
    "inject",
    "configFile",
    "config_file",
    "bootstrap",
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedManifestResult {
    pub manifest: Option<PluginManifest>,
    pub manifest_kind: Option<PluginManifestKind>,
    pub manifest_path: Option<String>,
    pub shadowed_manifest_path: Option<String>,
    pub diagnostics: Vec<PluginDiagnostic>,
}

// Original: manifest.ts, parseManifest(). Filesystem waits use Tokio while
// field parsing remains local and ordered.
pub async fn parse_manifest(plugin_root: impl AsRef<Path>) -> ParsedManifestResult {
    let plugin_root = plugin_root.as_ref();
    let root_json_path = plugin_root.join(KIMI_PLUGIN_ROOT_PATH);
    let dir_json_path = plugin_root.join(KIMI_PLUGIN_DIR_PATH);
    let root_json_exists = is_file(&root_json_path).await;
    let dir_json_exists = is_file(&dir_json_path).await;

    if !root_json_exists && !dir_json_exists {
        return ParsedManifestResult {
            diagnostics: vec![diagnostic(
                PluginDiagnosticSeverity::Error,
                format!("No manifest at {KIMI_PLUGIN_ROOT_PATH} or {KIMI_PLUGIN_DIR_PATH}"),
            )],
            ..ParsedManifestResult::default()
        };
    }

    let (manifest_path, manifest_kind) = if root_json_exists {
        (&root_json_path, PluginManifestKind::KimiPluginRoot)
    } else {
        (&dir_json_path, PluginManifestKind::KimiPluginDir)
    };
    let shadowed_manifest_path =
        (root_json_exists && dir_json_exists).then(|| path_to_string(&dir_json_path));
    let base_result = || ParsedManifestResult {
        manifest: None,
        manifest_kind: Some(manifest_kind),
        manifest_path: Some(path_to_string(manifest_path)),
        shadowed_manifest_path: shadowed_manifest_path.clone(),
        diagnostics: Vec::new(),
    };

    let raw = match tokio::fs::read_to_string(manifest_path).await {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(raw) => raw,
            Err(error) => {
                let mut result = base_result();
                result.diagnostics.push(diagnostic(
                    PluginDiagnosticSeverity::Error,
                    format!(
                        "Failed to parse {}: {error}",
                        relative_display(plugin_root, manifest_path)
                    ),
                ));
                return result;
            }
        },
        Err(error) => {
            let mut result = base_result();
            result.diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Error,
                format!(
                    "Failed to parse {}: {error}",
                    relative_display(plugin_root, manifest_path)
                ),
            ));
            return result;
        }
    };

    let Some(raw) = raw.as_object() else {
        let mut result = base_result();
        result.diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Error,
            "manifest must be a JSON object",
        ));
        return result;
    };

    let mut diagnostics = Vec::new();
    let name = raw
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if name.is_empty() {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Error,
            "\"name\" is required",
        ));
        let mut result = base_result();
        result.diagnostics = diagnostics;
        return result;
    }
    if !is_valid_plugin_name(name) {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Error,
            format!("\"name\" must match /^[a-z0-9][a-z0-9_-]{{0,63}}$/ (got \"{name}\")"),
        ));
        let mut result = base_result();
        result.diagnostics = diagnostics;
        return result;
    }

    let mut skills = resolve_skills_field(plugin_root, raw.get("skills"), &mut diagnostics).await;
    if !raw.contains_key("skills") && is_file(&plugin_root.join("SKILL.md")).await {
        skills = vec![path_to_string(plugin_root)];
    }
    record_unsupported_runtime_fields(raw, &mut diagnostics);

    let manifest = PluginManifest {
        name: name.to_owned(),
        version: string_field(raw, "version"),
        description: string_field(raw, "description"),
        keywords: string_array_field(raw, "keywords"),
        author: read_author(raw.get("author")),
        homepage: string_field(raw, "homepage"),
        license: string_field(raw, "license"),
        skills: Some(skills),
        session_start: read_session_start(raw.get("sessionStart"), &mut diagnostics),
        mcp_servers: read_mcp_servers(plugin_root, raw.get("mcpServers"), &mut diagnostics).await,
        hooks: read_hooks(raw.get("hooks"), &mut diagnostics),
        commands: read_commands(plugin_root, raw.get("commands"), &mut diagnostics).await,
        interface: read_interface(raw.get("interface")),
        skill_instructions: raw
            .get("skillInstructions")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };
    let mut result = base_result();
    result.manifest = Some(manifest);
    result.diagnostics = diagnostics;
    result
}

fn record_unsupported_runtime_fields(raw: &Map<String, Value>, out: &mut Vec<PluginDiagnostic>) {
    for field in UNSUPPORTED_RUNTIME_FIELDS {
        if raw.contains_key(field) {
            out.push(diagnostic(
                PluginDiagnosticSeverity::Info,
                format!("\"{field}\" is present but not supported by Kimi plugins"),
            ));
        }
    }
}

async fn resolve_skills_field(
    plugin_root: &Path,
    raw: Option<&Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Vec<String> {
    let Some(raw) = raw else { return Vec::new() };
    let entries = match string_or_string_array(raw) {
        Some(entries) => entries,
        None => {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Error,
                "\"skills\" must be a string or string[]",
            ));
            return Vec::new();
        }
    };
    let mut resolved = Vec::new();
    for entry in entries {
        if !entry.starts_with("./") {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Error,
                format!("\"skills\" path must start with \"./\" (got \"{entry}\")"),
            ));
            continue;
        }
        let absolute = resolve_lexical(plugin_root, entry);
        let real = tokio::fs::canonicalize(&absolute).await.unwrap_or(absolute);
        let root_real = tokio::fs::canonicalize(plugin_root)
            .await
            .unwrap_or_else(|_| plugin_root.to_owned());
        if !is_within(&real, &root_real) {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Error,
                format!("\"skills\" path resolves outside the plugin ({entry})"),
            ));
            continue;
        }
        if !is_dir(&real).await {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Warn,
                format!("\"skills\" path is not a directory ({entry})"),
            ));
            continue;
        }
        resolved.push(path_to_string(real));
    }
    resolved
}

async fn resolve_plugin_path_field(
    plugin_root: &Path,
    field: &str,
    value: &str,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<PathBuf> {
    if !value.starts_with("./") {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            format!("\"{field}\" path must start with \"./\" (got \"{value}\")"),
        ));
        return None;
    }
    let absolute = resolve_lexical(plugin_root, value);
    let real = tokio::fs::canonicalize(&absolute).await.unwrap_or(absolute);
    let root_real = tokio::fs::canonicalize(plugin_root)
        .await
        .unwrap_or_else(|_| plugin_root.to_owned());
    if !is_within(&real, &root_real) {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            format!("\"{field}\" path resolves outside the plugin ({value})"),
        ));
        return None;
    }
    Some(real)
}

fn read_session_start(
    raw: Option<&Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<PluginSessionStart> {
    let raw = raw?;
    let Some(raw) = raw.as_object() else {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            "\"sessionStart\" must be an object",
        ));
        return None;
    };
    let skill = raw
        .get("skill")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if skill.is_empty() {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            "\"sessionStart.skill\" is required when sessionStart is present",
        ));
        return None;
    }
    Some(PluginSessionStart {
        skill: skill.to_owned(),
    })
}

async fn read_mcp_servers(
    plugin_root: &Path,
    raw: Option<&Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<HashMap<String, McpServerConfig>> {
    let raw = raw?;
    let Some(raw) = raw.as_object() else {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            "\"mcpServers\" must be an object",
        ));
        return None;
    };
    let mut out = HashMap::new();
    for (name, value) in raw {
        let name = name.trim();
        if name.is_empty() {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Warn,
                "\"mcpServers\" entries must have a non-empty name",
            ));
            continue;
        }
        let config = match parse_mcp_server_config(value) {
            Ok(config) => config,
            Err(error) => {
                diagnostics.push(diagnostic(
                    PluginDiagnosticSeverity::Warn,
                    format!("Invalid MCP server \"{name}\": {error}"),
                ));
                continue;
            }
        };
        if let Some(config) =
            normalize_plugin_mcp_server(plugin_root, name, config, diagnostics).await
        {
            out.insert(name.to_owned(), config);
        }
    }
    (!out.is_empty()).then_some(out)
}

async fn normalize_plugin_mcp_server(
    plugin_root: &Path,
    name: &str,
    config: McpServerConfig,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<McpServerConfig> {
    let McpServerConfig::Stdio(mut config) = config else {
        return Some(config);
    };
    if config.command.starts_with("./") {
        config.command = path_to_string(
            resolve_plugin_path_field(
                plugin_root,
                &format!("mcpServers.{name}.command"),
                &config.command,
                diagnostics,
            )
            .await?,
        );
    } else if config.command.contains('/') || Path::new(&config.command).is_absolute() {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            format!("\"mcpServers.{name}.command\" must be a PATH command or start with \"./\""),
        ));
        return None;
    }
    if let Some(cwd) = config.cwd.clone() {
        config.cwd = Some(path_to_string(
            resolve_plugin_path_field(
                plugin_root,
                &format!("mcpServers.{name}.cwd"),
                &cwd,
                diagnostics,
            )
            .await?,
        ));
    }
    Some(McpServerConfig::Stdio(config))
}

fn read_hooks(
    raw: Option<&Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<Vec<HookDefConfig>> {
    let raw = raw?;
    let Some(raw) = raw.as_array() else {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            "\"hooks\" must be an array",
        ));
        return None;
    };
    let mut out = Vec::new();
    for (index, entry) in raw.iter().enumerate() {
        match parse_hook_def_config(entry) {
            Ok(hook) => out.push(hook),
            Err(error) => diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Warn,
                format!("Invalid hook at index {index}: {error}"),
            )),
        }
    }
    (!out.is_empty()).then_some(out)
}

async fn read_commands(
    plugin_root: &Path,
    raw: Option<&Value>,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<Vec<PluginCommandEntry>> {
    let raw = raw?;
    let Some(entries) = string_or_string_array(raw) else {
        diagnostics.push(diagnostic(
            PluginDiagnosticSeverity::Warn,
            "\"commands\" must be a string or string[]",
        ));
        return None;
    };
    let mut files = Vec::new();
    for entry in entries {
        let Some(resolved) =
            resolve_plugin_path_field(plugin_root, "commands", entry, diagnostics).await
        else {
            continue;
        };
        if is_dir(&resolved).await {
            files.extend(list_markdown_files_recursive(&resolved).await);
        } else if is_file(&resolved).await && path_to_string(&resolved).ends_with(".md") {
            let parent = resolved.parent().unwrap_or(&resolved);
            files.push(PluginCommandEntry {
                path: path_to_string(&resolved),
                name: command_name_from_file(&resolved, parent),
            });
        } else {
            diagnostics.push(diagnostic(
                PluginDiagnosticSeverity::Warn,
                format!("\"commands\" entry must be a directory or .md file ({entry})"),
            ));
        }
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    (!files.is_empty()).then_some(files)
}

async fn list_markdown_files_recursive(root: &Path) -> Vec<PluginCommandEntry> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".md") {
                out.push(PluginCommandEntry {
                    name: command_name_from_file(&path, root),
                    path: path_to_string(path),
                });
            }
        }
    }
    out
}

fn command_name_from_file(file: &Path, root: &Path) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    let mut name = relative
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if name
        .get(name.len().saturating_sub(3)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
    {
        name.truncate(name.len() - 3);
    }
    name
}

fn read_author(raw: Option<&Value>) -> Option<PluginAuthor> {
    if let Some(name) = raw.and_then(Value::as_str) {
        return Some(PluginAuthor {
            name: Some(name.to_owned()),
            email: None,
        });
    }
    let raw = raw?.as_object()?;
    let author = PluginAuthor {
        name: string_field(raw, "name"),
        email: string_field(raw, "email"),
    };
    (author.name.is_some() || author.email.is_some()).then_some(author)
}

fn read_interface(raw: Option<&Value>) -> Option<PluginInterface> {
    let raw = raw?.as_object()?;
    let interface = PluginInterface {
        display_name: string_field(raw, "displayName"),
        short_description: string_field(raw, "shortDescription"),
        long_description: string_field(raw, "longDescription"),
        developer_name: string_field(raw, "developerName"),
        website_url: string_field(raw, "websiteURL"),
    };
    (interface.display_name.is_some()
        || interface.short_description.is_some()
        || interface.long_description.is_some()
        || interface.developer_name.is_some()
        || interface.website_url.is_some())
    .then_some(interface)
}

fn string_field(raw: &Map<String, Value>, key: &str) -> Option<String> {
    let value = raw.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn string_array_field(raw: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    raw.get(key)?
        .as_array()?
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect()
}

fn string_or_string_array(raw: &Value) -> Option<Vec<&str>> {
    if let Some(value) = raw.as_str() {
        return Some(vec![value]);
    }
    raw.as_array()?.iter().map(Value::as_str).collect()
}

fn resolve_lexical(root: &Path, value: &str) -> PathBuf {
    normalize_lexical(&root.join(value))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            component => out.push(component.as_os_str()),
        }
    }
    out
}

fn is_within(child: &Path, parent: &Path) -> bool {
    child == parent || child.strip_prefix(parent).is_ok()
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

async fn is_dir(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

fn diagnostic(severity: PluginDiagnosticSeverity, message: impl Into<String>) -> PluginDiagnostic {
    PluginDiagnostic {
        severity,
        message: message.into(),
    }
}

fn path_to_string(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().into_owned()
}

fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(path_to_string)
        .unwrap_or_else(|_| path_to_string(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_root() -> PathBuf {
        std::env::temp_dir().join(format!("plugin-manifest-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn reports_missing_and_invalid_manifests_without_throwing() {
        let root = temporary_root();
        tokio::fs::create_dir_all(&root).await.unwrap();
        let missing = parse_manifest(&root).await;
        assert!(missing.manifest.is_none());
        assert_eq!(
            missing.diagnostics[0].severity,
            PluginDiagnosticSeverity::Error
        );

        tokio::fs::write(root.join(KIMI_PLUGIN_ROOT_PATH), "[]")
            .await
            .unwrap();
        let invalid = parse_manifest(&root).await;
        assert_eq!(
            invalid.diagnostics[0].message,
            "manifest must be a JSON object"
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn root_manifest_shadows_directory_manifest_and_normalizes_contributions() {
        let root = temporary_root();
        tokio::fs::create_dir_all(root.join(".kimi-plugin"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("skills"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("commands/nested"))
            .await
            .unwrap();
        tokio::fs::create_dir_all(root.join("bin")).await.unwrap();
        tokio::fs::write(root.join("commands/z.md"), "z")
            .await
            .unwrap();
        tokio::fs::write(root.join("commands/nested/a.md"), "a")
            .await
            .unwrap();
        tokio::fs::write(root.join("bin/server"), "").await.unwrap();
        tokio::fs::write(root.join(KIMI_PLUGIN_DIR_PATH), r#"{"name":"shadow"}"#)
            .await
            .unwrap();
        tokio::fs::write(root.join(KIMI_PLUGIN_ROOT_PATH), r#"{
          "name":"demo", "skills":"./skills", "commands":"./commands",
          "mcpServers":{"local":{"command":"./bin/server","cwd":"./skills"},"remote":{"url":"https://example.com/mcp"}},
          "hooks":[{"event":"Stop","command":"cleanup"}],
          "sessionStart":{"skill":" start "}, "tools":[],
          "interface":{"websiteURL":" https://example.com "}
        }"#).await.unwrap();

        let parsed = parse_manifest(&root).await;
        let manifest = parsed.manifest.unwrap();
        assert_eq!(
            parsed.manifest_kind,
            Some(PluginManifestKind::KimiPluginRoot)
        );
        assert!(parsed.shadowed_manifest_path.is_some());
        assert_eq!(manifest.skills.as_ref().unwrap().len(), 1);
        assert_eq!(
            manifest
                .commands
                .as_ref()
                .unwrap()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["nested/a", "z"]
        );
        assert_eq!(
            manifest.mcp_servers.as_ref().unwrap()["local"].transport(),
            "stdio"
        );
        assert_eq!(
            manifest.mcp_servers.as_ref().unwrap()["remote"].transport(),
            "http"
        );
        assert_eq!(manifest.session_start.unwrap().skill, "start");
        assert!(
            parsed
                .diagnostics
                .iter()
                .any(|entry| entry.severity == PluginDiagnosticSeverity::Info)
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_outside_and_malformed_optional_contributions_but_keeps_manifest() {
        let root = temporary_root();
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(
            root.join(KIMI_PLUGIN_ROOT_PATH),
            r#"{
          "name":"demo", "skills":["../outside", "bad"], "commands":7,
          "mcpServers":{"bad":{"command":"path/to/server"}}, "hooks":[{"event":"Stop","command":""}]
        }"#,
        )
        .await
        .unwrap();
        let parsed = parse_manifest(&root).await;
        assert!(parsed.manifest.is_some());
        assert!(parsed.diagnostics.len() >= 4);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
