use std::{
    any::Any,
    collections::HashSet,
    sync::{Arc, Mutex},
};

use indexmap::IndexMap;

use crate::{
    sdk::{model_alias::ModelAlias, types::ThinkingEffort},
    tui::{
        components::{
            Component, ComponentRole,
            dialogs::model_selector::{
                ModelSelection, ModelSelectorComponent, ModelSelectorOptions, provider_display_name,
            },
            render::truncate_to_width,
        },
        keys::{EditorKey, matches_editor_key},
        theme::current_theme,
        utils::tab_strip::{RenderTabStripOptions, render_tab_strip},
    },
};

const ALL_TAB_ID: &str = "all";
const ALL_TAB_LABEL: &str = "All";

type SelectionCallback = dyn FnMut(ModelSelection) + Send;
type CancelCallback = dyn FnMut() + Send;
type SharedSelectionCallback = Arc<Mutex<Box<SelectionCallback>>>;
type SharedCancelCallback = Arc<Mutex<Box<CancelCallback>>>;

pub struct TabbedModelSelectorOptions {
    pub models: IndexMap<String, ModelAlias>,
    pub current_value: String,
    pub selected_value: Option<String>,
    pub current_thinking_effort: ThinkingEffort,
    pub initial_tab_id: Option<String>,
    pub warning: Option<String>,
    on_select: Box<SelectionCallback>,
    on_session_only_select: Option<Box<SelectionCallback>>,
    on_cancel: Box<CancelCallback>,
}

impl TabbedModelSelectorOptions {
    pub fn new<S, C>(
        models: IndexMap<String, ModelAlias>,
        current_value: impl Into<String>,
        current_thinking_effort: ThinkingEffort,
        on_select: S,
        on_cancel: C,
    ) -> Self
    where
        S: FnMut(ModelSelection) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            models,
            current_value: current_value.into(),
            selected_value: None,
            current_thinking_effort,
            initial_tab_id: None,
            warning: None,
            on_select: Box::new(on_select),
            on_session_only_select: None,
            on_cancel: Box::new(on_cancel),
        }
    }

    pub fn with_session_only_select<S>(mut self, callback: S) -> Self
    where
        S: FnMut(ModelSelection) + Send + 'static,
    {
        self.on_session_only_select = Some(Box::new(callback));
        self
    }
}

struct ModelTab {
    id: String,
    label: String,
    selector: ModelSelectorComponent,
}

/// Provider-tab wrapper around [`ModelSelectorComponent`].
///
/// Original: `tabbed-model-selector.ts`, `TabbedModelSelectorComponent`.
pub struct TabbedModelSelectorComponent {
    pub focused: bool,
    tabs: Vec<ModelTab>,
    active_index: usize,
}

impl TabbedModelSelectorComponent {
    pub fn new(options: TabbedModelSelectorOptions) -> Self {
        let initial_tab_id = options.initial_tab_id.clone();
        let tabs = build_tabs(options);
        let active_index = initial_tab_id
            .as_deref()
            .and_then(|initial| tabs.iter().position(|tab| tab.id == initial))
            .unwrap_or_default();
        let mut selector = Self {
            focused: false,
            tabs,
            active_index,
        };
        selector.sync_focus_to_active();
        selector
    }

    pub fn active_tab_id(&self) -> Option<&str> {
        self.tabs.get(self.active_index).map(|tab| tab.id.as_str())
    }

    pub fn active_selected_alias(&self) -> Option<&str> {
        self.tabs
            .get(self.active_index)
            .and_then(|tab| tab.selector.selected_alias())
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if self.tabs.len() > 1 && matches_editor_key(data, EditorKey::Tab) {
            self.active_index = (self.active_index + 1) % self.tabs.len();
            self.sync_focus_to_active();
            return;
        }
        if self.tabs.len() > 1 && matches_editor_key(data, EditorKey::ShiftTab) {
            self.active_index = (self.active_index + self.tabs.len() - 1) % self.tabs.len();
            self.sync_focus_to_active();
            return;
        }
        if let Some(active) = self.tabs.get_mut(self.active_index) {
            active.selector.handle_input_event(data);
        }
    }

    fn render_selector(&mut self, width: usize) -> Vec<String> {
        self.sync_focus_to_active();
        let Some(active) = self.tabs.get_mut(self.active_index) else {
            return Vec::new();
        };
        let inner = active.selector.render(width);
        if self.tabs.len() <= 1 {
            return bound_lines(inner, width);
        }
        let labels = self
            .tabs
            .iter()
            .map(|tab| tab.label.clone())
            .collect::<Vec<_>>();
        let strip = render_tab_strip(&RenderTabStripOptions {
            labels: &labels,
            active_index: self.active_index,
            width,
            colors: &current_theme().palette(),
        });
        let header_end = inner.iter().position(String::is_empty).unwrap_or(3);
        let split_at = header_end.min(inner.len().saturating_sub(1));
        let mut lines = Vec::with_capacity(inner.len() + 2);
        lines.extend(inner[..=split_at].iter().cloned());
        lines.push(strip);
        lines.push(String::new());
        lines.extend(inner[split_at + 1..].iter().cloned());
        bound_lines(lines, width)
    }

    fn sync_focus_to_active(&mut self) {
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            tab.selector.focused = self.focused && index == self.active_index;
        }
    }
}

impl Component for TabbedModelSelectorComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_selector(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.handle_input_event(data);
    }

    fn invalidate(&mut self) {
        for tab in &mut self.tabs {
            tab.selector.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn build_tabs(options: TabbedModelSelectorOptions) -> Vec<ModelTab> {
    let TabbedModelSelectorOptions {
        models,
        current_value,
        selected_value,
        current_thinking_effort,
        initial_tab_id: _,
        warning,
        on_select,
        on_session_only_select,
        on_cancel,
    } = options;
    let mut provider_ids = Vec::new();
    let mut seen = HashSet::new();
    for model in models.values() {
        if seen.insert(model.provider.clone()) {
            provider_ids.push(model.provider.clone());
        }
    }
    let on_select = Arc::new(Mutex::new(on_select));
    let on_session_only_select =
        on_session_only_select.map(|callback| Arc::new(Mutex::new(callback)));
    let on_cancel = Arc::new(Mutex::new(on_cancel));

    let context = SelectorContext {
        current_value: &current_value,
        selected_value: selected_value.as_deref(),
        current_thinking_effort: &current_thinking_effort,
        warning: warning.as_deref(),
        on_select: &on_select,
        on_session_only_select: on_session_only_select.as_ref(),
        on_cancel: &on_cancel,
    };
    let mut tabs = vec![ModelTab {
        id: ALL_TAB_ID.to_owned(),
        label: ALL_TAB_LABEL.to_owned(),
        selector: make_selector(models.clone(), &context),
    }];
    for provider_id in provider_ids {
        let subset = models
            .iter()
            .filter(|(_, model)| model.provider == provider_id)
            .map(|(alias, model)| (alias.clone(), model.clone()))
            .collect();
        tabs.push(ModelTab {
            id: provider_id.clone(),
            label: provider_display_name(&provider_id),
            selector: make_selector(subset, &context),
        });
    }
    tabs
}

struct SelectorContext<'a> {
    current_value: &'a str,
    selected_value: Option<&'a str>,
    current_thinking_effort: &'a ThinkingEffort,
    warning: Option<&'a str>,
    on_select: &'a SharedSelectionCallback,
    on_session_only_select: Option<&'a SharedSelectionCallback>,
    on_cancel: &'a SharedCancelCallback,
}

fn make_selector(
    models: IndexMap<String, ModelAlias>,
    context: &SelectorContext<'_>,
) -> ModelSelectorComponent {
    let selected_candidate = context.selected_value.unwrap_or(context.current_value);
    let selected_value = models
        .contains_key(selected_candidate)
        .then(|| selected_candidate.to_owned());
    let select_callback = Arc::clone(context.on_select);
    let cancel_callback = Arc::clone(context.on_cancel);
    let mut options = ModelSelectorOptions::new(
        models,
        context.current_value,
        context.current_thinking_effort.clone(),
        move |selection| invoke_selection(&select_callback, selection),
        move || invoke_cancel(&cancel_callback),
    );
    options.selected_value = selected_value;
    options.searchable = true;
    options.provider_switch_hint = true;
    options.warning = context.warning.map(str::to_owned);
    if let Some(callback) = context.on_session_only_select {
        let callback = Arc::clone(callback);
        options = options.with_session_only_select(move |selection| {
            invoke_selection(&callback, selection);
        });
    }
    ModelSelectorComponent::new(options)
}

fn invoke_selection(callback: &SharedSelectionCallback, selection: ModelSelection) {
    let mut callback = callback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    callback(selection);
}

fn invoke_cancel(callback: &SharedCancelCallback) {
    let mut callback = callback
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    callback();
}

fn bound_lines(lines: Vec<String>, width: usize) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| truncate_to_width(&line, width, "…", false))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::components::render::visible_width;

    use super::*;

    fn model(provider: &str, name: &str) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: name.to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: Some(vec!["thinking".to_owned()]),
            display_name: None,
            reasoning_key: None,
            protocol: None,
            adaptive_thinking: None,
            support_efforts: None,
            default_effort: None,
            beta_api: None,
            overrides: None,
        }
    }

    fn models() -> IndexMap<String, ModelAlias> {
        IndexMap::from([
            ("first".to_owned(), model("provider-b", "First")),
            ("second".to_owned(), model("provider-a", "Second")),
            ("third".to_owned(), model("provider-b", "Third")),
        ])
    }

    #[test]
    fn builds_all_and_deduplicated_provider_tabs_in_insertion_order() {
        let selector = TabbedModelSelectorComponent::new(TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            |_| {},
            || {},
        ));
        assert_eq!(
            selector
                .tabs
                .iter()
                .map(|tab| (tab.id.as_str(), tab.label.as_str()))
                .collect::<Vec<_>>(),
            [
                ("all", "All"),
                ("provider-b", "provider-b"),
                ("provider-a", "provider-a")
            ]
        );
        assert_eq!(selector.active_tab_id(), Some("all"));
        assert_eq!(selector.active_selected_alias(), Some("second"));
    }

    #[test]
    fn cycles_tabs_and_each_tab_selects_its_own_subset() {
        let mut selector = TabbedModelSelectorComponent::new(TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            |_| {},
            || {},
        ));
        selector.handle_input_event("\t");
        assert_eq!(selector.active_tab_id(), Some("provider-b"));
        assert_eq!(selector.active_selected_alias(), Some("first"));
        selector.handle_input_event("\u{1b}[Z");
        assert_eq!(selector.active_tab_id(), Some("all"));
        selector.handle_input_event("\u{1b}[Z");
        assert_eq!(selector.active_tab_id(), Some("provider-a"));
        assert_eq!(selector.active_selected_alias(), Some("second"));
    }

    #[test]
    fn honors_valid_initial_tab_and_falls_back_to_all() {
        let mut options = TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            |_| {},
            || {},
        );
        options.initial_tab_id = Some("provider-b".to_owned());
        let selector = TabbedModelSelectorComponent::new(options);
        assert_eq!(selector.active_tab_id(), Some("provider-b"));

        let mut options = TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            |_| {},
            || {},
        );
        options.initial_tab_id = Some("missing".to_owned());
        let selector = TabbedModelSelectorComponent::new(options);
        assert_eq!(selector.active_tab_id(), Some("all"));
    }

    #[test]
    fn forwards_selection_session_only_and_cancel_callbacks() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&events);
        let session = Arc::clone(&events);
        let cancelled = Arc::clone(&events);
        let options = TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            move |selection| {
                selected
                    .lock()
                    .expect("events")
                    .push(format!("select:{}", selection.alias));
            },
            move || cancelled.lock().expect("events").push("cancel".to_owned()),
        )
        .with_session_only_select(move |selection| {
            session
                .lock()
                .expect("events")
                .push(format!("session:{}", selection.alias));
        });
        let mut selector = TabbedModelSelectorComponent::new(options);
        selector.handle_input_event("\r");
        selector.handle_input_event("\u{1b}s");
        selector.handle_input_event("\u{1b}");
        assert_eq!(
            *events.lock().expect("events"),
            ["select:second", "session:second", "cancel"]
        );
    }

    #[test]
    fn renders_tab_strip_between_header_and_model_list() {
        let mut options = TabbedModelSelectorOptions::new(
            models(),
            "second",
            ThinkingEffort::from("on"),
            |_| {},
            || {},
        );
        options.warning = Some("Model switching warning".to_owned());
        let mut selector = TabbedModelSelectorComponent::new(options);
        let lines = selector.render(64);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        let strip_index = plain
            .iter()
            .position(|line| line.contains("All") && line.contains("provider-b"))
            .expect("tab strip");
        let model_index = plain
            .iter()
            .position(|line| line.contains("Second") && line.contains("provider-a"))
            .expect("model row");
        assert!(
            plain[..strip_index]
                .iter()
                .any(|line| line.contains("warning"))
        );
        assert!(strip_index < model_index);
        assert!(lines.iter().all(|line| visible_width(line) <= 64));
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
