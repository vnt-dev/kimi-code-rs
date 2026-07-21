use std::{any::Any, collections::HashMap};

use indexmap::IndexMap;
use serde_json::Value;
use url::Url;

use crate::{
    oauth::open_platform::{get_open_platform_by_id, is_open_platform_id},
    tui::{
        components::{
            Component, ComponentRole,
            render::{truncate_to_width, visible_width},
        },
        keys::{EditorKey, ListKey, matches_editor_key, matches_list_key},
        theme::{ColorToken, current_theme},
        utils::{paging::page_view, printable_key::printable_char},
    },
};

const DEFAULT_OAUTH_PROVIDER_NAME: &str = "managed:kimi-code";
const ADD_ROW_LABEL: &str = "[ Add New Platform ]";
const PAGE_SIZE: isize = 8;
const CURRENT_MARK: &str = "← current";
const SELECT_POINTER: &str = "❯";

type VoidCallback = dyn FnMut() + Send;
type DeleteCallback = dyn FnMut(Vec<String>) + Send;

pub struct ProviderManagerOptions {
    pub providers: IndexMap<String, Value>,
    pub active_provider_id: Option<String>,
    on_add: Box<VoidCallback>,
    on_delete_source: Box<DeleteCallback>,
    on_close: Box<VoidCallback>,
}

impl ProviderManagerOptions {
    pub fn new<A, D, C>(
        providers: IndexMap<String, Value>,
        on_add: A,
        on_delete_source: D,
        on_close: C,
    ) -> Self
    where
        A: FnMut() + Send + 'static,
        D: FnMut(Vec<String>) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            providers,
            active_provider_id: None,
            on_add: Box::new(on_add),
            on_delete_source: Box::new(on_delete_source),
            on_close: Box::new(on_close),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmState {
    label: String,
    provider_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderRow {
    Source {
        id: String,
        label: String,
        provider_ids: Vec<String>,
        has_active: bool,
        base_url: Option<String>,
    },
    Add,
}

impl ProviderRow {
    fn id(&self) -> &str {
        match self {
            Self::Source { id, .. } => id,
            Self::Add => "__add__",
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Source { label, .. } => label,
            Self::Add => ADD_ROW_LABEL,
        }
    }
}

/// Pure-view provider CRUD list.
///
/// Original: `provider-manager.ts`, `ProviderManagerComponent`.
pub struct ProviderManagerComponent {
    pub focused: bool,
    options: ProviderManagerOptions,
    rows: Vec<ProviderRow>,
    selected_index: usize,
    confirm: Option<ConfirmState>,
}

impl ProviderManagerComponent {
    pub fn new(options: ProviderManagerOptions) -> Self {
        let rows = build_rows(&options);
        let selected_index = options
            .active_provider_id
            .as_deref()
            .and_then(|active| {
                rows.iter().position(|row| {
                    matches!(
                        row,
                        ProviderRow::Source { provider_ids, .. }
                            if provider_ids.iter().any(|id| id == active)
                    )
                })
            })
            .unwrap_or_default();
        Self {
            focused: false,
            options,
            rows,
            selected_index,
            confirm: None,
        }
    }

    /// Original: `ProviderManagerComponent.setOptions()`.
    pub fn set_options(&mut self, options: ProviderManagerOptions) {
        let previous_id = self.rows.get(self.selected_index).map(ProviderRow::id);
        let previous_provider = self
            .rows
            .get(self.selected_index)
            .and_then(|row| match row {
                ProviderRow::Source { provider_ids, .. } => provider_ids.first(),
                ProviderRow::Add => None,
            });
        let previous_id = previous_id.map(str::to_owned);
        let previous_provider = previous_provider.cloned();
        self.options = options;
        self.rows = build_rows(&self.options);
        self.confirm = None;
        let by_id = previous_id
            .as_deref()
            .and_then(|id| self.rows.iter().position(|row| row.id() == id));
        let by_provider = previous_provider.as_deref().and_then(|provider| {
            self.rows.iter().position(|row| {
                matches!(
                    row,
                    ProviderRow::Source { provider_ids, .. }
                        if provider_ids.iter().any(|id| id == provider)
                )
            })
        });
        self.selected_index = by_id
            .or(by_provider)
            .unwrap_or_else(|| self.selected_index.min(self.rows.len().saturating_sub(1)));
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if self.confirm.is_some() {
            self.handle_confirm_input(data);
            return;
        }
        if matches_editor_key(data, EditorKey::Escape) {
            (self.options.on_close)();
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
                .min(self.rows.len().saturating_sub(1));
            return;
        }
        if matches_editor_key(data, EditorKey::Left) || matches_list_key(data, ListKey::PageUp) {
            self.selected_index = self.selected_index.saturating_sub(PAGE_SIZE as usize);
            return;
        }
        if matches_editor_key(data, EditorKey::Right) || matches_list_key(data, ListKey::PageDown) {
            self.selected_index = self
                .selected_index
                .saturating_add(PAGE_SIZE as usize)
                .min(self.rows.len().saturating_sub(1));
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            if matches!(self.rows.get(self.selected_index), Some(ProviderRow::Add)) {
                (self.options.on_add)();
            }
            return;
        }
        if matches!(printable_char(data).as_str(), "d" | "D") {
            self.arm_delete_confirm();
        }
    }

    fn arm_delete_confirm(&mut self) {
        let Some(ProviderRow::Source {
            label,
            provider_ids,
            ..
        }) = self.rows.get(self.selected_index)
        else {
            return;
        };
        let prompt = if provider_ids.len() == 1 {
            format!("Delete platform \"{label}\"?")
        } else {
            format!(
                "Delete platform \"{label}\" and all {} providers?",
                provider_ids.len()
            )
        };
        self.confirm = Some(ConfirmState {
            label: prompt,
            provider_ids: provider_ids.clone(),
        });
    }

    fn handle_confirm_input(&mut self, data: &str) {
        let key = printable_char(data);
        if matches_editor_key(data, EditorKey::Escape) || matches!(key.as_str(), "n" | "N") {
            self.confirm = None;
        } else if matches!(key.as_str(), "y" | "Y")
            && let Some(confirm) = self.confirm.take()
        {
            (self.options.on_delete_source)(confirm.provider_ids);
        }
    }

    fn render_manager(&self, width: usize) -> Vec<String> {
        let border = current_theme().fg(ColorToken::Primary, &"─".repeat(width));
        let mut lines = vec![
            border.clone(),
            current_theme().bold_fg(ColorToken::Primary, " Providers"),
            current_theme().fg(
                ColorToken::TextMuted,
                " ↑↓ navigate · D delete · Esc cancel",
            ),
            String::new(),
        ];
        let view = page_view(
            self.rows.len(),
            isize::try_from(self.selected_index).unwrap_or(isize::MAX),
            PAGE_SIZE,
        );
        for index in view.start..view.end {
            for line in render_row(&self.rows[index], index == self.selected_index, width) {
                lines.push(line);
            }
        }
        lines.push(String::new());
        if let Some(confirm) = &self.confirm {
            lines.push(
                current_theme().bold_fg(ColorToken::Warning, &format!("  {} [y/N]", confirm.label)),
            );
        } else if view.page_count > 1 {
            lines.push(current_theme().fg(
                ColorToken::TextMuted,
                &format!(" Page {}/{}", view.page + 1, view.page_count),
            ));
        }
        lines.push(border);
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }
}

impl Component for ProviderManagerComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_manager(width)
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

fn build_rows(options: &ProviderManagerOptions) -> Vec<ProviderRow> {
    let mut rows = Vec::new();
    let mut registry_indexes = HashMap::<String, usize>::new();
    for (id, config) in &options.providers {
        if id == DEFAULT_OAUTH_PROVIDER_NAME {
            continue;
        }
        let active = options.active_provider_id.as_deref() == Some(id);
        if is_open_platform_id(id) {
            rows.push(ProviderRow::Source {
                id: format!("open:{id}"),
                label: get_open_platform_by_id(id)
                    .map_or_else(|| id.clone(), |p| p.name.to_owned()),
                provider_ids: vec![id.clone()],
                has_active: active,
                base_url: None,
            });
            continue;
        }
        let base_url = config
            .get("baseUrl")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some((url, api_key)) = read_custom_registry_source(config) {
            let key = format!("{url}{api_key}");
            if let Some(index) = registry_indexes.get(&key).copied()
                && let Some(ProviderRow::Source {
                    provider_ids,
                    has_active,
                    ..
                }) = rows.get_mut(index)
            {
                provider_ids.push(id.clone());
                *has_active |= active;
                continue;
            }
            registry_indexes.insert(key.clone(), rows.len());
            rows.push(ProviderRow::Source {
                id: format!("custom:{key}"),
                label: source_url_label(url),
                provider_ids: vec![id.clone()],
                has_active: active,
                base_url,
            });
            continue;
        }
        rows.push(ProviderRow::Source {
            id: format!("provider:{id}"),
            label: id.clone(),
            provider_ids: vec![id.clone()],
            has_active: active,
            base_url,
        });
    }
    rows.push(ProviderRow::Add);
    rows
}

fn read_custom_registry_source(provider: &Value) -> Option<(&str, &str)> {
    let source = provider.get("source")?.as_object()?;
    (source.get("kind")?.as_str()? == "apiJson").then_some(())?;
    let url = source.get("url")?.as_str()?;
    (!url.is_empty()).then_some(())?;
    Some((url, source.get("apiKey")?.as_str()?))
}

fn source_url_label(value: &str) -> String {
    let Ok(url) = Url::parse(value) else {
        return value.to_owned();
    };
    let Some(host) = url.host_str() else {
        return value.to_owned();
    };
    let port = url
        .port()
        .map_or_else(String::new, |port| format!(":{port}"));
    format!("{host}{port}{}", url.path().trim_end_matches('/'))
}

fn render_row(row: &ProviderRow, selected: bool, width: usize) -> Vec<String> {
    let pointer = if selected { SELECT_POINTER } else { " " };
    let pointer = current_theme().fg(
        if selected {
            ColorToken::Primary
        } else {
            ColorToken::TextDim
        },
        &format!("{pointer} "),
    );
    let active = matches!(
        row,
        ProviderRow::Source {
            has_active: true,
            ..
        }
    );
    let marker = if active {
        format!(" {CURRENT_MARK}")
    } else {
        String::new()
    };
    let label_width = width.saturating_sub(4 + visible_width(&marker));
    let label = truncate_to_width(row.label(), label_width, "…", false);
    let styled_label = if selected {
        current_theme().bold_fg(ColorToken::Primary, &label)
    } else if matches!(row, ProviderRow::Add) {
        current_theme().fg(ColorToken::Primary, &label)
    } else {
        current_theme().fg(ColorToken::Text, &label)
    };
    let mut line = format!("  {pointer}{styled_label}");
    if active {
        line.push_str(&current_theme().fg(ColorToken::Success, &marker));
    }
    let mut lines = vec![line];
    if let ProviderRow::Source {
        base_url: Some(url),
        ..
    } = row
        && !url.is_empty()
    {
        let url = truncate_to_width(url, width.saturating_sub(6), "…", false);
        lines.push(current_theme().fg(ColorToken::TextMuted, &format!("      {url}")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use super::*;

    fn providers() -> IndexMap<String, Value> {
        IndexMap::from([
            (DEFAULT_OAUTH_PROVIDER_NAME.to_owned(), json!({})),
            ("moonshot-cn".to_owned(), json!({})),
            (
                "standalone".to_owned(),
                json!({ "baseUrl": "https://api.test/v1" }),
            ),
            (
                "custom-a".to_owned(),
                json!({ "source": { "kind": "apiJson", "url": "https://registry.test/api.json/", "apiKey": "secret" }, "baseUrl": "https://a.test" }),
            ),
            (
                "custom-b".to_owned(),
                json!({ "source": { "kind": "apiJson", "url": "https://registry.test/api.json/", "apiKey": "secret" } }),
            ),
        ])
    }

    #[test]
    fn groups_sources_hides_default_oauth_and_marks_active_group() {
        let mut options = ProviderManagerOptions::new(providers(), || {}, |_| {}, || {});
        options.active_provider_id = Some("custom-b".to_owned());
        let manager = ProviderManagerComponent::new(options);
        assert_eq!(manager.rows.len(), 4);
        assert!(
            matches!(&manager.rows[2], ProviderRow::Source { label, provider_ids, has_active: true, .. } if label == "registry.test/api.json" && provider_ids == &["custom-a", "custom-b"])
        );
        assert_eq!(manager.selected_index, 2);
    }

    #[test]
    fn add_delete_confirm_cancel_and_close_dispatch_callbacks() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let add = Arc::clone(&events);
        let delete = Arc::clone(&events);
        let close = Arc::clone(&events);
        let options = ProviderManagerOptions::new(
            providers(),
            move || add.lock().expect("events").push("add".to_owned()),
            move |ids| {
                delete
                    .lock()
                    .expect("events")
                    .push(format!("delete:{}", ids.join(",")))
            },
            move || close.lock().expect("events").push("close".to_owned()),
        );
        let mut manager = ProviderManagerComponent::new(options);
        manager.handle_input_event("d");
        manager.handle_input_event("x");
        assert!(manager.confirm.is_some());
        manager.handle_input_event("n");
        manager.handle_input_event("d");
        manager.handle_input_event("y");
        for _ in 0..10 {
            manager.handle_input_event("\u{1b}[B");
        }
        manager.handle_input_event("\r");
        manager.handle_input_event("\u{1b}");
        assert_eq!(
            *events.lock().expect("events"),
            ["delete:moonshot-cn", "add", "close"]
        );
    }

    #[test]
    fn set_options_preserves_row_and_clears_confirmation() {
        let mut manager = ProviderManagerComponent::new(ProviderManagerOptions::new(
            providers(),
            || {},
            |_| {},
            || {},
        ));
        manager.handle_input_event("\u{1b}[B");
        manager.handle_input_event("d");
        assert!(manager.confirm.is_some());
        manager.set_options(ProviderManagerOptions::new(
            providers(),
            || {},
            |_| {},
            || {},
        ));
        assert_eq!(
            manager.rows[manager.selected_index].id(),
            "provider:standalone"
        );
        assert!(manager.confirm.is_none());
    }

    #[test]
    fn renders_paging_base_url_current_marker_and_confirmation() {
        let mut many = providers();
        for index in 0..8 {
            many.insert(format!("extra-{index}"), json!({}));
        }
        let mut options = ProviderManagerOptions::new(many, || {}, |_| {}, || {});
        options.active_provider_id = Some("standalone".to_owned());
        let mut manager = ProviderManagerComponent::new(options);
        let plain = manager
            .render(52)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(
            plain
                .iter()
                .any(|line| line.contains("https://api.test/v1"))
        );
        assert!(plain.iter().any(|line| line.contains("← current")));
        assert!(plain.iter().any(|line| line.contains("Page 1/2")));
        manager.handle_input_event("d");
        let plain = manager
            .render(52)
            .iter()
            .map(|line| strip_sgr(line))
            .collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("[y/N]")));
    }

    fn strip_sgr(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
