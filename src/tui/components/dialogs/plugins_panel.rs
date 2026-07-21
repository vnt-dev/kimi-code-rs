use std::{
    any::Any,
    collections::{HashMap, HashSet},
};

use crate::{
    sdk::types::{PluginState, PluginSummary},
    tui::{
        components::{
            Component, ComponentRole, Input, InputAction,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
        utils::{
            plugin_source_label::{format_plugin_source_label, plugin_trust_label},
            printable_key::printable_char,
            tab_strip::{RenderTabStripOptions, render_tab_strip},
        },
    },
    utils::plugin_marketplace::{
        PluginMarketplaceEntry, PluginMarketplaceTier, PluginUpdateStatus, compute_update_status,
    },
};

const WEB_BRIDGE_URL: &str = "https://www.kimi.com/features/webbridge#local-agent";
const SELECT_POINTER: &str = "❯";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginsPanelTabId {
    Installed,
    Official,
    ThirdParty,
    Custom,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginsMarketStatus {
    Idle,
    Loading,
    Error,
    Loaded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsPanelSelection {
    Toggle { id: String, enabled: bool },
    Remove { id: String },
    Mcp { id: String },
    Details { id: String },
    Reload,
    Install { entry: PluginMarketplaceEntry },
    InstallSource { source: String },
    OpenUrl { url: String, label: String },
}

type SelectCallback = dyn FnMut(PluginsPanelSelection) + Send;
type VoidCallback = dyn FnMut() + Send;

pub struct PluginsPanelOptions {
    pub installed: Vec<PluginSummary>,
    pub installed_ids: HashSet<String>,
    pub initial_tab: PluginsPanelTabId,
    pub selected_id: Option<String>,
    pub plugin_hint: Option<(String, String)>,
    on_select: Box<SelectCallback>,
    on_cancel: Box<VoidCallback>,
    on_request_marketplace: Option<Box<VoidCallback>>,
}
impl PluginsPanelOptions {
    pub fn new<S, C>(
        installed: Vec<PluginSummary>,
        installed_ids: HashSet<String>,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(PluginsPanelSelection) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            installed,
            installed_ids,
            initial_tab: PluginsPanelTabId::Installed,
            selected_id: None,
            plugin_hint: None,
            on_select: Box::new(on_select),
            on_cancel: Box::new(on_cancel),
            on_request_marketplace: None,
        }
    }
    pub fn with_marketplace_request<R>(mut self, callback: R) -> Self
    where
        R: FnMut() + Send + 'static,
    {
        self.on_request_marketplace = Some(Box::new(callback));
        self
    }
}

#[derive(Debug, Clone)]
enum MarketState {
    Idle,
    Loading,
    Error(String),
    Loaded {
        entries: Vec<PluginMarketplaceEntry>,
        source: String,
    },
}
const TABS: [(PluginsPanelTabId, &str); 4] = [
    (PluginsPanelTabId::Installed, "Installed"),
    (PluginsPanelTabId::Official, "Official"),
    (PluginsPanelTabId::ThirdParty, "Third-party"),
    (PluginsPanelTabId::Custom, "Custom"),
];

/// Unified local and marketplace plugin manager.
///
/// Original: `plugins-selector.ts`, `PluginsPanelComponent`.
pub struct PluginsPanelComponent {
    pub focused: bool,
    options: PluginsPanelOptions,
    custom_input: Input,
    active_tab_index: usize,
    selected_index: usize,
    market: MarketState,
    installing: Option<String>,
}

impl PluginsPanelComponent {
    pub fn new(options: PluginsPanelOptions) -> Self {
        let active_tab_index = TABS
            .iter()
            .position(|(id, _)| *id == options.initial_tab)
            .unwrap_or_default();
        let selected_index = if options.initial_tab == PluginsPanelTabId::Installed {
            options
                .selected_id
                .as_deref()
                .and_then(|id| options.installed.iter().position(|plugin| plugin.id == id))
                .unwrap_or_default()
        } else {
            0
        };
        Self {
            focused: false,
            options,
            custom_input: Input::new(),
            active_tab_index,
            selected_index,
            market: MarketState::Idle,
            installing: None,
        }
    }
    pub fn active_tab(&self) -> PluginsPanelTabId {
        TABS[self.active_tab_index].0
    }
    pub fn marketplace_status(&self) -> PluginsMarketStatus {
        match self.market {
            MarketState::Idle => PluginsMarketStatus::Idle,
            MarketState::Loading => PluginsMarketStatus::Loading,
            MarketState::Error(_) => PluginsMarketStatus::Error,
            MarketState::Loaded { .. } => PluginsMarketStatus::Loaded,
        }
    }
    pub fn set_marketplace_loading(&mut self) {
        self.market = MarketState::Loading;
    }
    pub fn set_marketplace(
        &mut self,
        entries: Vec<PluginMarketplaceEntry>,
        source: impl Into<String>,
    ) {
        self.market = MarketState::Loaded {
            entries,
            source: source.into(),
        };
    }
    pub fn set_marketplace_error(&mut self, message: impl Into<String>) {
        self.market = MarketState::Error(message.into());
    }
    pub fn set_installing(&mut self, label: impl Into<String>) {
        self.installing = Some(label.into());
    }
    pub fn clear_installing(&mut self) {
        self.installing = None;
    }

    fn request_marketplace_if_needed(&mut self) {
        if matches!(self.market, MarketState::Idle)
            && self.active_tab() != PluginsPanelTabId::Custom
        {
            self.market = MarketState::Loading;
            if let Some(callback) = &mut self.options.on_request_marketplace {
                callback();
            }
        }
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Tab) {
            self.active_tab_index = (self.active_tab_index + 1) % TABS.len();
            self.selected_index = 0;
            self.request_marketplace_if_needed();
            return;
        }
        if matches_editor_key(data, EditorKey::ShiftTab) {
            self.active_tab_index = (self.active_tab_index + TABS.len() - 1) % TABS.len();
            self.selected_index = 0;
            self.request_marketplace_if_needed();
            return;
        }
        match self.active_tab() {
            PluginsPanelTabId::Installed => self.handle_installed_input(data),
            PluginsPanelTabId::Official | PluginsPanelTabId::ThirdParty => {
                self.handle_marketplace_input(data)
            }
            PluginsPanelTabId::Custom => {
                if let Some(InputAction::Submit(value)) = self.custom_input.handle_input_event(data)
                {
                    let source = value.trim();
                    if !source.is_empty() {
                        (self.options.on_select)(PluginsPanelSelection::InstallSource {
                            source: source.to_owned(),
                        });
                    }
                }
            }
        }
    }

    fn handle_installed_input(&mut self, data: &str) {
        let len = self.options.installed.len();
        if matches_editor_key(data, EditorKey::Up) {
            self.selected_index = self.selected_index.saturating_sub(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.selected_index = self
                .selected_index
                .saturating_add(1)
                .min(len.saturating_sub(1));
            return;
        }
        let plugin = self.options.installed.get(self.selected_index);
        let key = printable_char(data);
        let selection = if key == " " {
            plugin.map(|p| PluginsPanelSelection::Toggle {
                id: p.id.clone(),
                enabled: !p.enabled,
            })
        } else if matches!(key.as_str(), "d" | "D") {
            plugin.map(|p| PluginsPanelSelection::Remove { id: p.id.clone() })
        } else if matches!(key.as_str(), "m" | "M") {
            plugin.map(|p| PluginsPanelSelection::Mcp { id: p.id.clone() })
        } else if matches!(key.as_str(), "r" | "R") {
            Some(PluginsPanelSelection::Reload)
        } else if matches_editor_key(data, EditorKey::Enter) {
            plugin.map(|p| {
                self.installed_update(p).map_or_else(
                    || PluginsPanelSelection::Details { id: p.id.clone() },
                    |update| PluginsPanelSelection::Install { entry: update.0 },
                )
            })
        } else if matches!(key.as_str(), "i" | "I") {
            plugin.map(|p| PluginsPanelSelection::Details { id: p.id.clone() })
        } else {
            None
        };
        if let Some(selection) = selection {
            (self.options.on_select)(selection);
        }
    }

    fn handle_marketplace_input(&mut self, data: &str) {
        let entries = self.active_marketplace_rows();
        if matches_editor_key(data, EditorKey::Up) {
            self.selected_index = self.selected_index.saturating_sub(1);
            return;
        }
        if matches_editor_key(data, EditorKey::Down) {
            self.selected_index = if entries.is_empty() {
                0
            } else {
                self.selected_index.saturating_add(1).min(entries.len() - 1)
            };
            return;
        }
        if matches_editor_key(data, EditorKey::Enter)
            && let Some(row) = entries.get(self.selected_index)
        {
            let selection = if row.pinned {
                PluginsPanelSelection::OpenUrl {
                    url: WEB_BRIDGE_URL.to_owned(),
                    label: row.entry.display_name.clone(),
                }
            } else {
                PluginsPanelSelection::Install {
                    entry: row.entry.clone(),
                }
            };
            (self.options.on_select)(selection);
        }
    }

    fn marketplace_entries(&self) -> Vec<PluginMarketplaceEntry> {
        let MarketState::Loaded { entries, .. } = &self.market else {
            return Vec::new();
        };
        let mut entries = entries.clone();
        entries.sort_by_key(|entry| !self.options.installed_ids.contains(&entry.id));
        entries
    }
    fn active_marketplace_rows(&self) -> Vec<MarketRow> {
        match self.active_tab() {
            PluginsPanelTabId::Official => {
                let mut rows = vec![MarketRow {
                    entry: web_bridge_entry(),
                    pinned: true,
                }];
                rows.extend(
                    self.marketplace_entries()
                        .into_iter()
                        .filter(|entry| {
                            entry.tier == Some(PluginMarketplaceTier::Official)
                                && entry.id != "kimi-webbridge"
                        })
                        .map(|entry| MarketRow {
                            entry,
                            pinned: false,
                        }),
                );
                rows
            }
            PluginsPanelTabId::ThirdParty => self
                .marketplace_entries()
                .into_iter()
                .filter(|entry| entry.tier != Some(PluginMarketplaceTier::Official))
                .map(|entry| MarketRow {
                    entry,
                    pinned: false,
                })
                .collect(),
            _ => Vec::new(),
        }
    }
    fn installed_versions(&self) -> HashMap<&str, Option<&str>> {
        self.options
            .installed
            .iter()
            .map(|plugin| (plugin.id.as_str(), plugin.version.as_deref()))
            .collect()
    }
    fn installed_update(
        &self,
        plugin: &PluginSummary,
    ) -> Option<(PluginMarketplaceEntry, String, String)> {
        let MarketState::Loaded { entries, .. } = &self.market else {
            return None;
        };
        let entry = entries.iter().find(|entry| entry.id == plugin.id)?;
        match compute_update_status(entry.version.as_deref(), plugin.version.as_deref(), true) {
            PluginUpdateStatus::Update { local, latest } => Some((entry.clone(), local, latest)),
            _ => None,
        }
    }

    fn render_panel(&mut self, width: usize) -> Vec<String> {
        if let Some(label) = &self.installing {
            return bound(
                vec![
                    current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
                    current_theme().bold_fg(ColorToken::Primary, " Plugins"),
                    String::new(),
                    current_theme().fg(
                        ColorToken::TextMuted,
                        &format!("  Installing {label} from marketplace…"),
                    ),
                    String::new(),
                    current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
                ],
                width,
            );
        }
        let hint = match self.active_tab() {
            PluginsPanelTabId::Installed => self.installed_hint(),
            PluginsPanelTabId::Custom => " Tab switch · Enter install · Esc cancel".to_owned(),
            _ => " Tab switch · ↑↓ navigate · Enter open/install · Esc cancel".to_owned(),
        };
        let labels = TABS
            .iter()
            .map(|(_, label)| (*label).to_owned())
            .collect::<Vec<_>>();
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(ColorToken::Primary, " Plugins"),
            current_theme().fg(ColorToken::TextMuted, &hint),
            String::new(),
            render_tab_strip(&RenderTabStripOptions {
                labels: &labels,
                active_index: self.active_tab_index,
                width,
                colors: &current_theme().palette(),
            }),
            String::new(),
        ];
        match self.active_tab() {
            PluginsPanelTabId::Installed => self.render_installed(&mut lines, width),
            PluginsPanelTabId::Official => self.render_official(&mut lines, width),
            PluginsPanelTabId::ThirdParty => self.render_third_party(&mut lines, width),
            PluginsPanelTabId::Custom => self.render_custom(&mut lines, width),
        }
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        bound(lines, width)
    }
    fn installed_hint(&self) -> String {
        let update = self
            .options
            .installed
            .get(self.selected_index)
            .is_some_and(|plugin| self.installed_update(plugin).is_some());
        format!(
            " Tab switch · Space toggle · D remove · M MCP · Enter {} · I details · R reload · Esc cancel",
            if update { "update" } else { "details" }
        )
    }
    fn render_installed(&self, lines: &mut Vec<String>, width: usize) {
        if self.options.installed.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "  No plugins installed."));
        } else {
            for (index, plugin) in self.options.installed.iter().enumerate() {
                lines.extend(self.render_installed_row(plugin, index, width));
            }
        }
        lines.push(String::new());
        lines.push(current_theme().fg(
            ColorToken::TextMuted,
            &format!(" {} installed", self.options.installed.len()),
        ));
    }
    fn render_installed_row(
        &self,
        plugin: &PluginSummary,
        index: usize,
        width: usize,
    ) -> Vec<String> {
        let selected = index == self.selected_index;
        let mut line = row_prefix(selected, &plugin.display_name);
        let status = plugin_status(plugin);
        line.push_str("  ");
        line.push_str(&current_theme().fg(status_token(&status), &status));
        if let Some((_, local, latest)) = self.installed_update(plugin) {
            line.push_str("  ");
            line.push_str(
                &current_theme().fg(ColorToken::Warning, &format!("update {local} → {latest}")),
            );
        }
        if let Some((id, hint)) = &self.options.plugin_hint
            && id == &plugin.id
        {
            line.push_str("  ");
            line.push_str(&current_theme().fg(ColorToken::Warning, hint));
        }
        let description = overview_plugin_description(plugin);
        let mut lines = vec![line];
        for text in wrap_description(&description, width.saturating_sub(4).max(1)) {
            lines.push(current_theme().fg(ColorToken::TextMuted, &format!("    {text}")));
        }
        lines
    }
    fn render_official(&self, lines: &mut Vec<String>, width: usize) {
        lines.extend(self.render_marketplace_row(
            &MarketRow {
                entry: web_bridge_entry(),
                pinned: true,
            },
            0,
            width,
        ));
        let entries = self
            .active_marketplace_rows()
            .into_iter()
            .skip(1)
            .collect::<Vec<_>>();
        self.render_marketplace_state(lines, width, &entries, 1);
    }
    fn render_third_party(&self, lines: &mut Vec<String>, width: usize) {
        let entries = self.active_marketplace_rows();
        self.render_marketplace_state(lines, width, &entries, 0);
    }
    fn render_marketplace_state(
        &self,
        lines: &mut Vec<String>,
        width: usize,
        entries: &[MarketRow],
        offset: usize,
    ) {
        match &self.market {
            MarketState::Idle | MarketState::Loading => {
                lines.push(current_theme().fg(ColorToken::TextMuted, "  Loading marketplace…"));
                return;
            }
            MarketState::Error(message) => {
                lines.push(current_theme().fg(
                    ColorToken::Warning,
                    &format!("  Marketplace unavailable: {message}"),
                ));
                lines.push(current_theme().fg(
                    ColorToken::TextMuted,
                    "  Use the Custom tab to install from a URL.",
                ));
                return;
            }
            MarketState::Loaded { .. } => {}
        }
        if entries.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "  No plugins found."));
        } else {
            for (index, row) in entries.iter().enumerate() {
                lines.extend(self.render_marketplace_row(row, index + offset, width));
            }
        }
        let installed = entries
            .iter()
            .filter(|row| self.options.installed_ids.contains(&row.entry.id))
            .count();
        lines.push(String::new());
        lines.push(current_theme().fg(
            ColorToken::TextMuted,
            &format!(
                " {installed} installed · {} available",
                entries.len() - installed
            ),
        ));
        if let MarketState::Loaded { source, .. } = &self.market {
            lines.push(current_theme().fg(ColorToken::TextMuted, &format!(" Source: {source}")));
        }
    }
    fn render_marketplace_row(&self, row: &MarketRow, index: usize, width: usize) -> Vec<String> {
        let selected = index == self.selected_index;
        let status = if row.pinned {
            "open in browser".to_owned()
        } else {
            marketplace_entry_status(&row.entry, &self.installed_versions())
        };
        let mut line = row_prefix(selected, &row.entry.display_name);
        line.push_str("  ");
        line.push_str(&current_theme().fg(marketplace_status_token(&status), &status));
        let mut lines = vec![line];
        for text in wrap_description(
            &marketplace_entry_description(&row.entry),
            width.saturating_sub(4).max(1),
        ) {
            lines.push(current_theme().fg(ColorToken::TextMuted, &format!("    {text}")));
        }
        lines
    }
    fn render_custom(&mut self, lines: &mut Vec<String>, width: usize) {
        lines.push(current_theme().fg(
            ColorToken::TextMuted,
            " Install from a GitHub URL (or zip URL / local path):",
        ));
        lines.push(String::new());
        self.custom_input.focused = self.focused;
        let box_width = width.saturating_sub(2).max(24);
        let inner = box_width.saturating_sub(4).max(10);
        let input = self
            .custom_input
            .render(inner)
            .into_iter()
            .next()
            .unwrap_or_default();
        let pad = " ".repeat(inner.saturating_sub(visible_width(&input)));
        let border = |text: &str| current_theme().fg(ColorToken::Primary, text);
        lines.extend([
            format!(" {}", border(&format!("╭{}╮", "─".repeat(box_width - 2)))),
            format!(" {}  {input}{pad}{}", border("│"), border("│")),
            format!(" {}", border(&format!("╰{}╯", "─".repeat(box_width - 2)))),
        ]);
    }
}
impl Component for PluginsPanelComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_panel(width)
    }
    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }
    fn invalidate(&mut self) {
        self.custom_input.invalidate();
    }
    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
struct MarketRow {
    entry: PluginMarketplaceEntry,
    pinned: bool,
}
fn web_bridge_entry() -> PluginMarketplaceEntry {
    PluginMarketplaceEntry {
        id: "kimi-webbridge".into(),
        display_name: "Kimi WebBridge".into(),
        source: WEB_BRIDGE_URL.into(),
        tier: Some(PluginMarketplaceTier::Official),
        version: None,
        homepage: Some(WEB_BRIDGE_URL.into()),
        description: Some(
            "Control your real browser from Kimi Code — navigate, click, type, and screenshot"
                .into(),
        ),
        keywords: None,
    }
}
fn row_prefix(selected: bool, label: &str) -> String {
    let pointer = if selected { SELECT_POINTER } else { " " };
    format!(
        "{}{}",
        current_theme().fg(
            if selected {
                ColorToken::Primary
            } else {
                ColorToken::TextDim
            },
            &format!("  {pointer} ")
        ),
        if selected {
            current_theme().bold_fg(ColorToken::Primary, label)
        } else {
            current_theme().fg(ColorToken::Text, label)
        }
    )
}
fn plugin_status(plugin: &PluginSummary) -> String {
    if plugin.state != PluginState::Ok {
        match plugin.state {
            PluginState::Ok => String::new(),
            PluginState::Error => "error".into(),
        }
    } else if plugin.enabled {
        "enabled".into()
    } else {
        "disabled".into()
    }
}
fn status_token(status: &str) -> ColorToken {
    if status == "enabled" || status == "installed" {
        ColorToken::Success
    } else if status == "disabled" {
        ColorToken::TextDim
    } else {
        ColorToken::Warning
    }
}
fn marketplace_status_token(status: &str) -> ColorToken {
    if status.starts_with("update") {
        ColorToken::Warning
    } else if status.starts_with("installed") {
        ColorToken::Success
    } else {
        ColorToken::Primary
    }
}
fn overview_plugin_description(plugin: &PluginSummary) -> String {
    let state = if plugin.state == PluginState::Ok {
        String::new()
    } else {
        " · state error".into()
    };
    let mcp = if plugin.mcp_server_count > 0 {
        format!(
            " · MCP {}/{}",
            plugin.enabled_mcp_server_count, plugin.mcp_server_count
        )
    } else {
        String::new()
    };
    let diagnostics = if plugin.has_errors {
        " · diagnostics available"
    } else {
        ""
    };
    format!(
        "id {} · {} skill{}{} · {} · {}{}{}",
        plugin.id,
        plugin.skill_count,
        if plugin.skill_count == 1 { "" } else { "s" },
        mcp,
        format_plugin_source_label(plugin),
        plugin_trust_label(plugin).as_str(),
        state,
        diagnostics
    )
}
fn marketplace_entry_description(entry: &PluginMarketplaceEntry) -> String {
    let tier = match entry.tier {
        Some(PluginMarketplaceTier::Official) => "Official plugin",
        Some(PluginMarketplaceTier::Curated) => "Curated plugin",
        None => "Plugin",
    };
    let description = entry.description.as_deref().unwrap_or(tier);
    let version = entry
        .version
        .as_deref()
        .map(|version| format!(" · v{version}"))
        .unwrap_or_default();
    let tier_suffix = entry
        .description
        .as_ref()
        .map(|_| format!(" · {tier}"))
        .unwrap_or_default();
    let keywords = entry
        .keywords
        .as_ref()
        .filter(|words| !words.is_empty())
        .map(|words| format!(" · {}", words.join(", ")))
        .unwrap_or_default();
    format!(
        "{description} · id {}{version}{tier_suffix}{keywords}",
        entry.id
    )
}
fn marketplace_entry_status(
    entry: &PluginMarketplaceEntry,
    installed: &HashMap<&str, Option<&str>>,
) -> String {
    match compute_update_status(
        entry.version.as_deref(),
        installed.get(entry.id.as_str()).copied().flatten(),
        installed.contains_key(entry.id.as_str()),
    ) {
        PluginUpdateStatus::Update { local, latest } => format!("update {local} → {latest}"),
        PluginUpdateStatus::UpToDate { version } => version.map_or_else(
            || "installed".into(),
            |version| format!("installed · v{version}"),
        ),
        PluginUpdateStatus::NotInstalled => entry
            .version
            .as_deref()
            .map_or_else(|| "install".into(), |version| format!("install v{version}")),
    }
}
fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.into()
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
                word.into()
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
fn bound(lines: Vec<String>, width: usize) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "…", false))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::types::{PluginSource, PluginState};
    use std::sync::{Arc, Mutex};
    fn plugin(id: &str, version: Option<&str>) -> PluginSummary {
        PluginSummary {
            id: id.into(),
            display_name: format!("Plugin {id}"),
            version: version.map(str::to_owned),
            enabled: true,
            state: PluginState::Ok,
            skill_count: 1,
            mcp_server_count: 1,
            enabled_mcp_server_count: 1,
            hook_count: 0,
            command_count: 0,
            has_errors: false,
            source: PluginSource::LocalPath,
            original_source: None,
            github: None,
        }
    }
    fn entry(
        id: &str,
        tier: Option<PluginMarketplaceTier>,
        version: Option<&str>,
    ) -> PluginMarketplaceEntry {
        PluginMarketplaceEntry {
            id: id.into(),
            display_name: format!("Market {id}"),
            source: format!("https://plugins/{id}"),
            tier,
            version: version.map(str::to_owned),
            description: Some("Useful plugin".into()),
            homepage: None,
            keywords: None,
        }
    }
    fn panel(events: Arc<Mutex<Vec<PluginsPanelSelection>>>) -> PluginsPanelComponent {
        let called = Arc::clone(&events);
        PluginsPanelComponent::new(PluginsPanelOptions::new(
            vec![plugin("a", Some("1.0.0"))],
            HashSet::from(["a".into()]),
            move |value| called.lock().expect("events").push(value),
            || {},
        ))
    }
    #[test]
    fn installed_actions_and_updates_dispatch() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut panel = panel(Arc::clone(&events));
        panel.set_marketplace(
            vec![entry(
                "a",
                Some(PluginMarketplaceTier::Official),
                Some("2.0.0"),
            )],
            "catalog",
        );
        panel.handle_input_event(" ");
        panel.handle_input_event("d");
        panel.handle_input_event("m");
        panel.handle_input_event("i");
        panel.handle_input_event("r");
        panel.handle_input_event("\r");
        assert!(
            matches!(events.lock().expect("events").last(),Some(PluginsPanelSelection::Install{entry})if entry.id=="a")
        );
        assert_eq!(events.lock().expect("events").len(), 6);
    }
    #[test]
    fn tabs_request_marketplace_pin_webbridge_and_submit_custom_source() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(0));
        let called = Arc::clone(&events);
        let requested = Arc::clone(&requests);
        let options = PluginsPanelOptions::new(
            Vec::new(),
            HashSet::new(),
            move |value| called.lock().expect("events").push(value),
            || {},
        )
        .with_marketplace_request(move || *requested.lock().expect("requests") += 1);
        let mut panel = PluginsPanelComponent::new(options);
        panel.handle_input_event("\t");
        assert_eq!(*requests.lock().expect("requests"), 1);
        panel.handle_input_event("\r");
        panel.handle_input_event("\t");
        panel.handle_input_event("\t");
        for c in "https://github.com/x/y".chars() {
            panel.handle_input_event(&c.to_string());
        }
        panel.handle_input_event("\r");
        assert!(
            matches!(&events.lock().expect("events")[0],PluginsPanelSelection::OpenUrl{url,..}if url==WEB_BRIDGE_URL)
        );
        assert!(
            matches!(&events.lock().expect("events")[1],PluginsPanelSelection::InstallSource{source}if source=="https://github.com/x/y")
        );
    }
    #[test]
    fn categorizes_dedupes_and_orders_marketplace_rows() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut panel = panel(events);
        panel.set_marketplace(
            vec![
                entry("third", None, None),
                entry("a", Some(PluginMarketplaceTier::Official), Some("1.0.0")),
                entry(
                    "kimi-webbridge",
                    Some(PluginMarketplaceTier::Official),
                    None,
                ),
                entry("official", Some(PluginMarketplaceTier::Official), None),
            ],
            "catalog",
        );
        panel.active_tab_index = 1;
        let official = panel.active_marketplace_rows();
        assert_eq!(
            official
                .iter()
                .map(|row| row.entry.id.as_str())
                .collect::<Vec<_>>(),
            ["kimi-webbridge", "a", "official"]
        );
        panel.active_tab_index = 2;
        assert_eq!(panel.active_marketplace_rows()[0].entry.id, "third");
    }
    #[test]
    fn renders_all_states_and_installing_with_width_bound() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut panel = panel(events);
        panel.set_marketplace_error("offline");
        panel.active_tab_index = 1;
        let lines = panel.render(64);
        assert!(
            lines
                .iter()
                .any(|line| strip(line).contains("Marketplace unavailable"))
        );
        assert!(lines.iter().all(|line| visible_width(line) <= 64));
        panel.set_installing("Example");
        assert!(
            panel
                .render(40)
                .iter()
                .any(|line| strip(line).contains("Installing Example"))
        );
    }
    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
