use crate::{
    sdk::types::{
        McpServerTransport, PluginDiagnosticSeverity, PluginGithubRefKind, PluginInfo,
        PluginManifestKind, PluginSource, PluginState, PluginSummary,
    },
    tui::{
        theme::{ColorToken, current_theme},
        utils::plugin_source_label::{
            CURATED_BADGE, OFFICIAL_BADGE, PluginTrustLabel, THIRD_PARTY_BADGE,
            format_plugin_source_label, plugin_trust_label,
        },
    },
};

pub fn build_plugins_list_lines(plugins: &[PluginSummary]) -> Vec<String> {
    let theme = current_theme();
    if plugins.is_empty() {
        return vec![
            theme.fg(ColorToken::TextDim, "No plugins installed."),
            String::new(),
            theme.fg(ColorToken::Text, "Run /plugins to install one."),
        ];
    }

    let mut lines = Vec::new();
    for plugin in plugins {
        let enabled = if plugin.enabled {
            theme.fg(ColorToken::Success, "enabled")
        } else {
            theme.fg(ColorToken::TextDim, "disabled")
        };
        let state = if plugin.state == PluginState::Ok {
            String::new()
        } else {
            format!(" [{}]", state_label(plugin.state))
        };
        let version = plugin.version.as_deref().unwrap_or("-");
        let diagnostics = if plugin.has_errors {
            theme.fg(ColorToken::Warning, " | diagnostics: see /plugins info")
        } else {
            String::new()
        };
        let source_tag = theme.fg(
            ColorToken::TextDim,
            &format!("[{}]", format_plugin_source_label(plugin)),
        );
        let trust_badge = render_trust_badge(plugin_trust_label(plugin));
        lines.push(format!(
            "{} ({}) {} {} {} | {enabled}{state}",
            theme.fg(ColorToken::Text, &plugin.display_name),
            theme.fg(ColorToken::TextDim, &plugin.id),
            theme.fg(ColorToken::TextDim, version),
            source_tag,
            trust_badge,
        ));
        let mcp = if plugin.mcp_server_count > 0 {
            format!(
                " | {}/{} mcp",
                plugin.enabled_mcp_server_count, plugin.mcp_server_count
            )
        } else {
            String::new()
        };
        lines.push(format!(
            "  {} {}{}{}",
            theme.fg(ColorToken::TextDim, "skills:"),
            theme.fg(ColorToken::Text, &plugin.skill_count.to_string()),
            theme.fg(ColorToken::TextDim, &mcp),
            diagnostics
        ));
    }
    lines
}

fn render_trust_badge(label: PluginTrustLabel) -> String {
    let (token, badge) = match label {
        PluginTrustLabel::Official => (ColorToken::Success, OFFICIAL_BADGE),
        PluginTrustLabel::Curated => (ColorToken::Primary, CURATED_BADGE),
        PluginTrustLabel::ThirdParty => (ColorToken::TextDim, THIRD_PARTY_BADGE),
    };
    current_theme().fg(token, &format!("[{badge}]"))
}

pub fn build_plugins_info_lines(info: &PluginInfo) -> Vec<String> {
    let theme = current_theme();
    let status = if info.enabled {
        theme.fg(ColorToken::Success, "enabled")
    } else {
        theme.fg(ColorToken::TextDim, "disabled")
    };
    let trust_line = match plugin_trust_label(info) {
        PluginTrustLabel::Official => format!(
            "{}  {} {}",
            theme.fg(ColorToken::TextDim, "Trust:"),
            theme.fg(ColorToken::Success, OFFICIAL_BADGE),
            theme.fg(ColorToken::TextDim, "(Kimi-built and -maintained)")
        ),
        PluginTrustLabel::Curated => format!(
            "{}  {} {}",
            theme.fg(ColorToken::TextDim, "Trust:"),
            theme.fg(ColorToken::Primary, CURATED_BADGE),
            theme.fg(ColorToken::TextDim, "(Kimi-reviewed, upstream-maintained)")
        ),
        PluginTrustLabel::ThirdParty => format!(
            "{}  {}",
            theme.fg(ColorToken::TextDim, "Trust:"),
            theme.fg(ColorToken::TextDim, THIRD_PARTY_BADGE)
        ),
    };
    let mut title = format!(
        "{} ({})",
        theme.fg(ColorToken::Text, &info.display_name),
        theme.fg(ColorToken::TextDim, &info.id)
    );
    if let Some(version) = info.version.as_deref().filter(|value| !value.is_empty()) {
        title.push(' ');
        title.push_str(&theme.fg(ColorToken::TextDim, version));
    }
    let mut lines = vec![
        title,
        format!(
            "{} {status} | {} {}",
            theme.fg(ColorToken::TextDim, "Status:"),
            theme.fg(ColorToken::TextDim, "state:"),
            state_text(info.state)
        ),
        trust_line,
        format!(
            "{} {}",
            theme.fg(ColorToken::TextDim, "Source:"),
            theme.fg(ColorToken::Text, info.source.as_str())
        ),
        format!(
            "{}   {}",
            theme.fg(ColorToken::TextDim, "Root:"),
            theme.fg(ColorToken::Text, &info.root)
        ),
    ];

    if info.source == PluginSource::Github
        && let Some(github) = &info.github
    {
        let reference = format!(
            "{}:{}",
            github_ref_kind(github.reference.kind),
            github.reference.value
        );
        lines.push(format!(
            "{} {} {}",
            theme.fg(ColorToken::TextDim, "GitHub:"),
            theme.fg(
                ColorToken::Text,
                &format!("{}/{}", github.owner, github.repo)
            ),
            theme.fg(ColorToken::TextDim, &format!("@{reference}"))
        ));
        if let Some(sha) = &github.installed_sha {
            lines.push(format!(
                "{} {}",
                theme.fg(ColorToken::TextDim, "Installed SHA:"),
                theme.fg(ColorToken::Text, sha)
            ));
        }
    }
    if let Some(source) = &info.original_source {
        lines.push(field_line("Original source:", source));
    }
    lines.push(field_line("Installed at:", &info.installed_at));
    if let Some(updated) = info
        .updated_at
        .as_deref()
        .filter(|updated| *updated != info.installed_at)
    {
        lines.push(field_line("Last updated:", updated));
    }
    if let Some(path) = &info.manifest_path {
        let suffix = info.manifest_kind.map_or_else(String::new, |kind| {
            format!(
                " {}",
                theme.fg(ColorToken::TextDim, &format!("({})", manifest_kind(kind)))
            )
        });
        lines.push(format!(
            "{} {}{suffix}",
            theme.fg(ColorToken::TextDim, "Manifest:"),
            theme.fg(ColorToken::Text, path)
        ));
    }
    if let Some(path) = &info.shadowed_manifest_path {
        lines.push(field_line("Shadowed:", path));
    }
    if let Some(skill) = info
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.session_start.as_ref())
        .map(|start| start.skill.as_str())
    {
        lines.push(field_line("Session start:", skill));
    }
    if info
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.skill_instructions.as_ref())
        .is_some()
    {
        lines.push(field_line("Skill instructions:", "present"));
    }

    let skills = info
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.skills.as_deref())
        .unwrap_or_default();
    lines.push(String::new());
    lines.push(theme.fg(ColorToken::Text, &format!("Skills ({}):", skills.len())));
    for directory in skills {
        lines.push(format!(
            "  {} {}",
            theme.fg(ColorToken::TextDim, "-"),
            theme.fg(ColorToken::Text, directory)
        ));
    }

    if !info.mcp_servers.is_empty() {
        lines.push(String::new());
        lines.push(theme.fg(
            ColorToken::Text,
            &format!(
                "MCP servers ({}/{} enabled):",
                info.enabled_mcp_server_count, info.mcp_server_count
            ),
        ));
        lines.push(theme.fg(
            ColorToken::TextDim,
            &format!(
                "  Enabled by default; disable with /plugins mcp disable {} <server>.",
                info.id
            ),
        ));
        for server in &info.mcp_servers {
            let enabled = if server.enabled {
                theme.fg(ColorToken::Success, "enabled")
            } else {
                theme.fg(ColorToken::TextDim, "disabled")
            };
            lines.push(format!(
                "  {} {} {enabled} {}",
                theme.fg(ColorToken::TextDim, "-"),
                theme.fg(ColorToken::Text, &server.name),
                theme.fg(ColorToken::TextDim, &format!("({})", server.runtime_name))
            ));
            if server.transport == McpServerTransport::Stdio {
                let command = std::iter::once(server.command.as_deref().unwrap_or_default())
                    .chain(
                        server
                            .args
                            .as_deref()
                            .unwrap_or_default()
                            .iter()
                            .map(String::as_str),
                    )
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                lines.push(format!(
                    "    {} {}",
                    theme.fg(ColorToken::TextDim, "command:"),
                    theme.fg(ColorToken::Text, command.trim())
                ));
                if let Some(cwd) = &server.cwd {
                    lines.push(indented_field_line("cwd:", cwd));
                }
                if let Some(keys) = server.env_keys.as_deref().filter(|keys| !keys.is_empty()) {
                    lines.push(indented_field_line("env:", &keys.join(", ")));
                }
            } else {
                lines.push(indented_field_line(
                    "url:",
                    server.url.as_deref().unwrap_or_default(),
                ));
                if let Some(keys) = server
                    .header_keys
                    .as_deref()
                    .filter(|keys| !keys.is_empty())
                {
                    lines.push(indented_field_line("headers:", &keys.join(", ")));
                }
            }
        }
    }

    if let Some(interface) = info
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.interface_config.as_ref())
    {
        lines.push(String::new());
        lines.push(theme.fg(ColorToken::Text, "Display:"));
        let mut display_values = Vec::new();
        if let Some(description) = &interface.short_description {
            display_values.push(description.clone());
        }
        if let Some(name) = &interface.developer_name {
            display_values.push(format!("by {name}"));
        }
        if let Some(url) = &interface.website_url {
            display_values.push(url.clone());
        }
        for value in display_values {
            lines.push(format!(
                "  {} {}",
                theme.fg(ColorToken::TextDim, "-"),
                theme.fg(ColorToken::Text, &value)
            ));
        }
    }
    if let Some(keywords) = info
        .manifest
        .as_ref()
        .and_then(|manifest| manifest.keywords.as_deref())
        .filter(|keywords| !keywords.is_empty())
    {
        lines.push(String::new());
        lines.push(theme.fg(
            ColorToken::TextDim,
            &format!("Keywords: {}", keywords.join(", ")),
        ));
    }
    if !info.diagnostics.is_empty() {
        lines.push(String::new());
        lines.push(theme.fg(ColorToken::Text, "Diagnostics:"));
        for diagnostic in &info.diagnostics {
            let token = match diagnostic.severity {
                PluginDiagnosticSeverity::Error => ColorToken::Error,
                PluginDiagnosticSeverity::Warn => ColorToken::Warning,
                PluginDiagnosticSeverity::Info => ColorToken::TextDim,
            };
            lines.push(format!(
                "  {} {}",
                theme.fg(
                    token,
                    &format!("[{}]", diagnostic_severity(diagnostic.severity))
                ),
                theme.fg(ColorToken::Text, &diagnostic.message)
            ));
        }
    }
    lines
}

fn field_line(label: &str, value: &str) -> String {
    format!(
        "{} {}",
        current_theme().fg(ColorToken::TextDim, label),
        current_theme().fg(ColorToken::Text, value)
    )
}

fn indented_field_line(label: &str, value: &str) -> String {
    format!("    {}", field_line(label, value))
}

fn state_text(state: PluginState) -> String {
    current_theme().fg(
        if state == PluginState::Ok {
            ColorToken::Success
        } else {
            ColorToken::Error
        },
        state_label(state),
    )
}

fn state_label(state: PluginState) -> &'static str {
    match state {
        PluginState::Ok => "ok",
        PluginState::Error => "error",
    }
}

fn github_ref_kind(kind: PluginGithubRefKind) -> &'static str {
    match kind {
        PluginGithubRefKind::Branch => "branch",
        PluginGithubRefKind::Tag => "tag",
        PluginGithubRefKind::Sha => "sha",
    }
}

fn manifest_kind(kind: PluginManifestKind) -> &'static str {
    match kind {
        PluginManifestKind::KimiPluginRoot => "kimi-plugin-root",
        PluginManifestKind::KimiPluginDir => "kimi-plugin-dir",
    }
}

fn diagnostic_severity(severity: PluginDiagnosticSeverity) -> &'static str {
    match severity {
        PluginDiagnosticSeverity::Error => "error",
        PluginDiagnosticSeverity::Warn => "warn",
        PluginDiagnosticSeverity::Info => "info",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::{PluginDiagnostic, PluginInterface, PluginManifest};

    fn strip_sgr(text: &str) -> String {
        let mut output = String::new();
        let mut escape = false;
        for character in text.chars() {
            if character == '\u{1b}' {
                escape = true;
            } else if escape && character == 'm' {
                escape = false;
            } else if !escape {
                output.push(character);
            }
        }
        output
    }

    #[test]
    fn renders_empty_and_populated_plugin_lists() {
        let empty = build_plugins_list_lines(&[])
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert_eq!(
            empty,
            ["No plugins installed.", "", "Run /plugins to install one."]
        );

        let plugin = PluginSummary {
            id: "demo".to_owned(),
            display_name: "Demo".to_owned(),
            version: Some("1.0.0".to_owned()),
            enabled: true,
            state: PluginState::Error,
            skill_count: 2,
            mcp_server_count: 1,
            enabled_mcp_server_count: 1,
            hook_count: 0,
            command_count: 0,
            has_errors: true,
            source: PluginSource::ZipUrl,
            original_source: Some(
                "https://code.kimi.com/kimi-code/plugins/official/demo.zip".to_owned(),
            ),
            github: None,
        };
        let plain = build_plugins_list_lines(&[plugin])
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(plain[0].contains("[official] | enabled [error]"));
        assert!(plain[1].contains("skills: 2 | 1/1 mcp | diagnostics"));
    }

    #[test]
    fn renders_manifest_display_keywords_and_diagnostics() {
        let info = PluginInfo {
            id: "demo".to_owned(),
            display_name: "Demo".to_owned(),
            version: Some("1.0.0".to_owned()),
            enabled: true,
            state: PluginState::Ok,
            skill_count: 1,
            mcp_server_count: 0,
            enabled_mcp_server_count: 0,
            hook_count: 0,
            command_count: 0,
            has_errors: false,
            source: PluginSource::LocalPath,
            original_source: None,
            github: None,
            root: "/plugins/demo".to_owned(),
            installed_at: "today".to_owned(),
            updated_at: None,
            manifest_kind: Some(PluginManifestKind::KimiPluginRoot),
            manifest_path: Some("/plugins/demo/kimi-plugin.json".to_owned()),
            manifest: Some(PluginManifest {
                name: "demo".to_owned(),
                version: None,
                description: None,
                keywords: Some(vec!["tools".to_owned(), "demo".to_owned()]),
                author: None,
                homepage: None,
                license: None,
                skills: Some(vec!["review".to_owned()]),
                session_start: None,
                mcp_servers: None,
                hooks: None,
                commands: None,
                interface_config: Some(PluginInterface {
                    short_description: Some("Short".to_owned()),
                    developer_name: Some("Moonshot".to_owned()),
                    website_url: Some("https://example.com".to_owned()),
                    ..PluginInterface::default()
                }),
                skill_instructions: Some("instructions".to_owned()),
            }),
            mcp_servers: Vec::new(),
            shadowed_manifest_path: None,
            diagnostics: vec![PluginDiagnostic {
                severity: PluginDiagnosticSeverity::Warn,
                message: "check config".to_owned(),
            }],
        };
        let plain = build_plugins_info_lines(&info)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Trust:  third-party",
            "Skills (1):",
            "by Moonshot",
            "Keywords: tools, demo",
            "[warn] check config",
        ] {
            assert!(
                plain.contains(expected),
                "missing {expected:?} in {plain:?}"
            );
        }
    }
}
