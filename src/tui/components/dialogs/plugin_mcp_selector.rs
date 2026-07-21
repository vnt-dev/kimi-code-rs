use std::any::Any;

use crate::{
    sdk::types::{McpServerTransport, PluginInfo, PluginMcpServerInfo},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
        utils::printable_key::printable_char,
    },
};

const MCP_SERVER_PREFIX: &str = "mcp:";
const SELECT_POINTER: &str = "❯";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginMcpSelection {
    Toggle {
        plugin_id: String,
        server: String,
        enabled: bool,
    },
    Back {
        plugin_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OverviewItem {
    value: String,
    action: bool,
    label: String,
    status: Option<String>,
    description: String,
}

type SelectCallback = dyn FnMut(PluginMcpSelection) + Send;
type CancelCallback = dyn FnMut() + Send;

pub struct PluginMcpSelectorOptions {
    pub info: PluginInfo,
    pub selected_server: Option<String>,
    pub server_hint: Option<(String, String)>,
    on_select: Box<SelectCallback>,
    on_cancel: Box<CancelCallback>,
}

impl PluginMcpSelectorOptions {
    pub fn new<S, C>(info: PluginInfo, on_select: S, on_cancel: C) -> Self
    where
        S: FnMut(PluginMcpSelection) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            info,
            selected_server: None,
            server_hint: None,
            on_select: Box::new(on_select),
            on_cancel: Box::new(on_cancel),
        }
    }
}

/// Per-plugin MCP server enable/disable list.
///
/// Original: `plugins-selector.ts`, `PluginMcpSelectorComponent`.
pub struct PluginMcpSelectorComponent {
    pub focused: bool,
    options: PluginMcpSelectorOptions,
    items: Vec<OverviewItem>,
    selected_index: usize,
}

impl PluginMcpSelectorComponent {
    pub fn new(options: PluginMcpSelectorOptions) -> Self {
        let items = build_mcp_items(&options.info);
        let selected_index = options
            .selected_server
            .as_deref()
            .and_then(|name| {
                items
                    .iter()
                    .position(|item| item.value == format!("{MCP_SERVER_PREFIX}{name}"))
            })
            .unwrap_or_default();
        Self {
            focused: false,
            options,
            items,
            selected_index,
        }
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Up) {
            self.selected_index = self.selected_index.saturating_sub(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.selected_index = self
                .selected_index
                .saturating_add(1)
                .min(self.items.len().saturating_sub(1));
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) || printable_char(data) == " " {
            let Some(chosen) = self.items.get(self.selected_index) else {
                return;
            };
            if chosen.value == "back" {
                (self.options.on_select)(PluginMcpSelection::Back {
                    plugin_id: self.options.info.id.clone(),
                });
                return;
            }
            let Some(server_name) = chosen.value.strip_prefix(MCP_SERVER_PREFIX) else {
                return;
            };
            if let Some(server) = self
                .options
                .info
                .mcp_servers
                .iter()
                .find(|server| server.name == server_name)
            {
                (self.options.on_select)(PluginMcpSelection::Toggle {
                    plugin_id: self.options.info.id.clone(),
                    server: server.name.clone(),
                    enabled: !server.enabled,
                });
            }
        }
    }

    fn render_selector(&self, width: usize) -> Vec<String> {
        let server_count = self.items.iter().filter(|item| !item.action).count();
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(
                ColorToken::Primary,
                &format!(" MCP servers · {}", self.options.info.display_name),
            ),
            current_theme().fg(
                ColorToken::TextMuted,
                " ↑↓ navigate · Enter/Space enable/disable · Esc cancel",
            ),
            String::new(),
            current_theme().bold_fg(
                ColorToken::TextDim,
                &format!(
                    " MCP servers ({}/{} enabled)",
                    self.options.info.enabled_mcp_server_count, self.options.info.mcp_server_count
                ),
            ),
        ];
        if server_count == 0 {
            lines.push(current_theme().fg(ColorToken::TextMuted, "  No MCP servers declared."));
        } else {
            for index in 0..server_count {
                lines.extend(self.render_item(&self.items[index], index, width));
            }
        }
        lines.push(String::new());
        lines.push(current_theme().bold_fg(ColorToken::TextDim, " Actions"));
        for index in server_count..self.items.len() {
            lines.extend(self.render_item(&self.items[index], index, width));
        }
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }

    fn render_item(&self, item: &OverviewItem, index: usize, width: usize) -> Vec<String> {
        let selected = index == self.selected_index;
        let pointer = if selected { SELECT_POINTER } else { " " };
        let mut line = current_theme().fg(
            if selected {
                ColorToken::Primary
            } else {
                ColorToken::TextDim
            },
            &format!("  {pointer} "),
        );
        let label = if selected {
            current_theme().bold_fg(ColorToken::Primary, &item.label)
        } else {
            current_theme().fg(ColorToken::Text, &item.label)
        };
        line.push_str(&label);
        if let Some(status) = &item.status {
            line.push_str("  ");
            line.push_str(&current_theme().fg(
                if status == "enabled" {
                    ColorToken::Success
                } else {
                    ColorToken::TextDim
                },
                status,
            ));
        }
        if let Some((server, hint)) = &self.options.server_hint
            && item.value == format!("{MCP_SERVER_PREFIX}{server}")
        {
            line.push_str("  ");
            line.push_str(&current_theme().fg(ColorToken::Warning, hint));
        }
        let mut lines = vec![line];
        for description in wrap_description(&item.description, width.saturating_sub(4).max(1)) {
            lines.push(current_theme().fg(ColorToken::TextMuted, &format!("    {description}")));
        }
        lines
    }
}

impl Component for PluginMcpSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_selector(width)
    }
    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }
    fn invalidate(&mut self) {}
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn build_mcp_items(info: &PluginInfo) -> Vec<OverviewItem> {
    let mut items = info
        .mcp_servers
        .iter()
        .map(|server| OverviewItem {
            value: format!("{MCP_SERVER_PREFIX}{}", server.name),
            action: false,
            label: server.name.clone(),
            status: Some(
                if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
                .to_owned(),
            ),
            description: mcp_server_description(server),
        })
        .collect::<Vec<_>>();
    items.push(OverviewItem {
        value: "back".to_owned(),
        action: true,
        label: "Back to installed plugins".to_owned(),
        status: None,
        description: "Return to the local plugin manager.".to_owned(),
    });
    items
}
fn mcp_server_description(server: &PluginMcpServerInfo) -> String {
    let action = if server.enabled {
        "Enter/Space disable"
    } else {
        "Enter/Space enable"
    };
    match server.transport {
        McpServerTransport::Http | McpServerTransport::Sse => format!(
            "{action} · {} · {}",
            if server.transport == McpServerTransport::Http {
                "HTTP"
            } else {
                "SSE"
            },
            server.url.as_deref().unwrap_or(&server.runtime_name)
        ),
        McpServerTransport::Stdio => {
            let args = server
                .args
                .as_ref()
                .filter(|args| !args.is_empty())
                .map(|args| format!(" {}", args.join(" ")))
                .unwrap_or_default();
            let command = format!("{}{args}", server.command.as_deref().unwrap_or_default())
                .trim()
                .to_owned();
            let cwd = server
                .cwd
                .as_deref()
                .map(|cwd| format!(" · cwd {cwd}"))
                .unwrap_or_default();
            format!(
                "{action} · stdio · {}{cwd}",
                if command.is_empty() {
                    &server.runtime_name
                } else {
                    &command
                }
            )
        }
    }
}
fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if visible_width(&candidate) <= width {
            current = candidate;
        } else {
            if !current.is_empty() {
                lines.push(current);
            }
            current = if visible_width(word) <= width {
                word.to_owned()
            } else {
                truncate_to_width(word, width, "…", false)
            };
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::{PluginSource, PluginState};
    use std::sync::{Arc, Mutex};
    fn info() -> PluginInfo {
        PluginInfo {
            id: "plug".into(),
            display_name: "Plugin".into(),
            version: None,
            enabled: true,
            state: PluginState::Ok,
            skill_count: 0,
            mcp_server_count: 2,
            enabled_mcp_server_count: 1,
            hook_count: 0,
            command_count: 0,
            has_errors: false,
            source: PluginSource::LocalPath,
            original_source: None,
            github: None,
            root: "/plugin".into(),
            installed_at: "now".into(),
            updated_at: None,
            manifest_kind: None,
            manifest_path: None,
            manifest: None,
            mcp_servers: vec![
                PluginMcpServerInfo {
                    name: "stdio".into(),
                    runtime_name: "plug:stdio".into(),
                    enabled: true,
                    transport: McpServerTransport::Stdio,
                    command: Some("node".into()),
                    args: Some(vec!["server.js".into()]),
                    cwd: Some("/plugin".into()),
                    url: None,
                    env_keys: None,
                    header_keys: None,
                },
                PluginMcpServerInfo {
                    name: "http".into(),
                    runtime_name: "plug:http".into(),
                    enabled: false,
                    transport: McpServerTransport::Http,
                    command: None,
                    args: None,
                    cwd: None,
                    url: Some("https://mcp.test".into()),
                    env_keys: None,
                    header_keys: None,
                },
            ],
            shadowed_manifest_path: None,
            diagnostics: Vec::new(),
        }
    }
    #[test]
    fn toggles_servers_and_returns_back() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut selector = PluginMcpSelectorComponent::new(PluginMcpSelectorOptions::new(
            info(),
            move |value| called.lock().expect("events").push(value),
            || {},
        ));
        selector.handle_input_event(" ");
        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event("\r");
        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event("\r");
        assert_eq!(
            *events.lock().expect("events"),
            [
                PluginMcpSelection::Toggle {
                    plugin_id: "plug".into(),
                    server: "stdio".into(),
                    enabled: false
                },
                PluginMcpSelection::Toggle {
                    plugin_id: "plug".into(),
                    server: "http".into(),
                    enabled: true
                },
                PluginMcpSelection::Back {
                    plugin_id: "plug".into()
                }
            ]
        );
    }
    #[test]
    fn renders_descriptions_hint_and_width_bounds() {
        let mut options = PluginMcpSelectorOptions::new(info(), |_| {}, || {});
        options.selected_server = Some("http".into());
        options.server_hint = Some(("http".into(), "restart required".into()));
        let mut selector = PluginMcpSelectorComponent::new(options);
        let lines = selector.render(48);
        let plain = lines.iter().map(|line| strip(line)).collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("MCP servers · Plugin"))
        );
        assert!(plain.iter().any(|line| line.contains("restart required")));
        assert!(plain.iter().any(|line| line.contains("node server.js")));
        assert!(lines.iter().all(|line| visible_width(line) <= 48));
    }
    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
