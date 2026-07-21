use std::{any::Any, collections::HashMap};

use indexmap::IndexMap;

use crate::{
    sdk::{
        model_alias::{ModelAlias, effective_model_alias},
        types::ThinkingEffort,
    },
    tui::{
        components::{
            Component, ComponentRole,
            dialogs::choice_picker::ChoiceOption,
            render::{truncate_to_width, visible_width, wrap_text_with_ansi},
        },
        keys::{EditorKey, matches_editor_key},
        theme::{ColorToken, current_theme},
        utils::searchable_list::{SearchableList, SearchableListView},
    },
};

const DEFAULT_OAUTH_PROVIDER_NAME: &str = "managed:kimi-code";
const PRODUCT_NAME: &str = "Kimi Code";
const CURRENT_MARK: &str = "← current";
const SELECT_POINTER: &str = "❯";

type SelectionCallback = dyn FnMut(ModelSelection) + Send;
type CancelCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingAvailability {
    Toggle,
    AlwaysOn,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelChoice {
    alias: String,
    model: ModelAlias,
    name: String,
    provider: String,
    label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub alias: String,
    pub thinking: ThinkingEffort,
}

pub struct ModelSelectorOptions {
    pub models: IndexMap<String, ModelAlias>,
    pub current_value: String,
    pub selected_value: Option<String>,
    pub current_thinking_effort: ThinkingEffort,
    pub searchable: bool,
    pub page_size: Option<isize>,
    pub provider_switch_hint: bool,
    pub warning: Option<String>,
    on_select: Box<SelectionCallback>,
    on_session_only_select: Option<Box<SelectionCallback>>,
    on_cancel: Box<CancelCallback>,
}

impl ModelSelectorOptions {
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
            searchable: false,
            page_size: None,
            provider_switch_hint: false,
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

/// Original: `model-selector.ts`, `modelDisplayName()`.
pub fn model_display_name(alias: &str, model: Option<&ModelAlias>) -> String {
    let effective = model.map(|model| effective_model_alias(model, None));
    effective
        .as_ref()
        .and_then(|model| model.display_name.clone())
        .or_else(|| effective.as_ref().map(|model| model.model.clone()))
        .unwrap_or_else(|| alias.to_owned())
}

/// Original: `model-selector.ts`, `providerDisplayName()`.
pub fn provider_display_name(provider: &str) -> String {
    if provider == DEFAULT_OAUTH_PROVIDER_NAME {
        PRODUCT_NAME.to_owned()
    } else if let Some(managed) = provider.strip_prefix("managed:") {
        managed.to_owned()
    } else {
        provider.to_owned()
    }
}

/// Original: `model-selector.ts`, `createModelChoiceOptions()`.
pub fn create_model_choice_options(models: &IndexMap<String, ModelAlias>) -> Vec<ChoiceOption> {
    models
        .iter()
        .map(|(alias, model)| {
            let effective = effective_model_alias(model, None);
            ChoiceOption::new(
                alias,
                format!(
                    "{} ({})",
                    model_display_name(alias, Some(&effective)),
                    provider_display_name(&effective.provider)
                ),
            )
        })
        .collect()
}

fn create_model_choices(models: &IndexMap<String, ModelAlias>) -> Vec<ModelChoice> {
    models
        .iter()
        .map(|(alias, model)| {
            let model = effective_model_alias(model, None);
            let name = model_display_name(alias, Some(&model));
            let provider = provider_display_name(&model.provider);
            ModelChoice {
                alias: alias.clone(),
                label: format!("{name} ({provider})"),
                model,
                name,
                provider,
            }
        })
        .collect()
}

/// Original: `model-selector.ts`, `thinkingAvailability()`.
pub fn thinking_availability(model: &ModelAlias) -> ThinkingAvailability {
    let capabilities = model.capabilities.as_deref().unwrap_or_default();
    if capabilities.iter().any(|value| value == "always_thinking") {
        ThinkingAvailability::AlwaysOn
    } else if capabilities.iter().any(|value| value == "thinking")
        || model.adaptive_thinking == Some(true)
    {
        ThinkingAvailability::Toggle
    } else {
        ThinkingAvailability::Unsupported
    }
}

/// Original: `model-selector.ts`, `effortsOf()`.
pub fn efforts_of(model: &ModelAlias) -> &[String] {
    model.support_efforts.as_deref().unwrap_or_default()
}

/// Original: `model-selector.ts`, `segmentsFor()`.
pub fn segments_for(model: &ModelAlias) -> Vec<String> {
    let efforts = efforts_of(model);
    let availability = thinking_availability(model);
    if !efforts.is_empty() {
        let mut segments = Vec::with_capacity(efforts.len() + 1);
        if availability != ThinkingAvailability::AlwaysOn {
            segments.push("off".to_owned());
        }
        segments.extend_from_slice(efforts);
        return segments;
    }
    match availability {
        ThinkingAvailability::AlwaysOn => vec!["on".to_owned()],
        ThinkingAvailability::Unsupported => vec!["off".to_owned()],
        ThinkingAvailability::Toggle => vec!["on".to_owned(), "off".to_owned()],
    }
}

/// Original: `model-selector.ts`, `effortLabel()`.
pub fn effort_label(effort: &str) -> String {
    let Some(first) = effort.chars().next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), &effort[first.len_utf8()..])
}

fn default_thinking_effort_for(model: &ModelAlias) -> ThinkingEffort {
    if thinking_availability(model) == ThinkingAvailability::Unsupported {
        return ThinkingEffort::from("off");
    }
    let efforts = efforts_of(model);
    if !efforts.is_empty() {
        return ThinkingEffort::new(
            model
                .default_effort
                .as_deref()
                .unwrap_or(&efforts[efforts.len() / 2]),
        );
    }
    ThinkingEffort::from("on")
}

fn commit_effort(choice: &ModelChoice, draft: &str) -> ThinkingEffort {
    if draft == "on" {
        default_thinking_effort_for(&choice.model)
    } else {
        ThinkingEffort::new(draft)
    }
}

/// Flat searchable model picker with an inline per-model thinking selector.
///
/// Original: `model-selector.ts`, `ModelSelectorComponent`.
pub struct ModelSelectorComponent {
    pub focused: bool,
    options: ModelSelectorOptions,
    list: SearchableList<ModelChoice>,
    thinking_overrides: HashMap<String, String>,
}

impl ModelSelectorComponent {
    pub fn new(options: ModelSelectorOptions) -> Self {
        let choices = create_model_choices(&options.models);
        let selected_value = options
            .selected_value
            .as_deref()
            .unwrap_or(&options.current_value);
        let selected_index = choices
            .iter()
            .position(|choice| choice.alias == selected_value)
            .unwrap_or_default();
        let list = SearchableList::new(
            choices,
            |choice: &ModelChoice| choice.label.clone(),
            options.page_size,
            Some(isize::try_from(selected_index).unwrap_or(isize::MAX)),
            options.searchable,
        );
        Self {
            focused: false,
            options,
            list,
            thinking_overrides: HashMap::new(),
        }
    }

    pub fn selected_alias(&self) -> Option<&str> {
        self.list.selected().map(|choice| choice.alias.as_str())
    }

    pub fn query(&self) -> &str {
        self.list.view().query
    }

    fn draft_for(&self, choice: &ModelChoice) -> String {
        if let Some(value) = self.thinking_overrides.get(&choice.alias) {
            return value.clone();
        }
        if choice.alias == self.options.current_value {
            return self.options.current_thinking_effort.as_str().to_owned();
        }
        let efforts = efforts_of(&choice.model);
        if !efforts.is_empty() {
            let default = choice
                .model
                .default_effort
                .as_ref()
                .unwrap_or(&efforts[efforts.len() / 2]);
            if efforts.contains(default) {
                return default.clone();
            }
            return efforts[0].clone();
        }
        if thinking_availability(&choice.model) == ThinkingAvailability::Unsupported {
            "off".to_owned()
        } else {
            "on".to_owned()
        }
    }

    fn effective_effort(&self, choice: &ModelChoice) -> String {
        let draft = self.draft_for(choice);
        let segments = segments_for(&choice.model);
        if segments.contains(&draft) {
            draft
        } else {
            segments[0].clone()
        }
    }

    fn selected_selection(&self) -> Option<ModelSelection> {
        let selected = self.list.selected()?;
        Some(ModelSelection {
            alias: selected.alias.clone(),
            thinking: commit_effort(selected, &self.effective_effort(selected)),
        })
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            if !self.list.clear_query() {
                (self.options.on_cancel)();
            }
            return;
        }
        if self.list.handle_key(data) {
            return;
        }
        if matches_editor_key(data, EditorKey::Left) || matches_editor_key(data, EditorKey::Right) {
            let Some(selected) = self.list.selected() else {
                return;
            };
            let segments = segments_for(&selected.model);
            if segments.len() <= 1 {
                return;
            }
            let current = self.effective_effort(selected);
            let index = segments
                .iter()
                .position(|segment| segment == &current)
                .unwrap_or_default();
            let next = if segments.len() == 2 {
                usize::from(index == 0)
            } else if matches_editor_key(data, EditorKey::Left) {
                index.saturating_sub(1)
            } else {
                index.saturating_add(1).min(segments.len() - 1)
            };
            if next != index {
                self.thinking_overrides
                    .insert(selected.alias.clone(), segments[next].clone());
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            if let Some(selection) = self.selected_selection() {
                (self.options.on_select)(selection);
            }
            return;
        }
        if matches_editor_key(data, EditorKey::Alt('s'))
            && let Some(selection) = self.selected_selection()
            && let Some(callback) = &mut self.options.on_session_only_select
        {
            callback(selection);
        }
    }

    fn render_selector(&self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let searchable = self.options.searchable;
        let view = self.list.view();
        let title_suffix = if searchable && view.query.is_empty() {
            current_theme().fg(ColorToken::TextMuted, "  (type to search)")
        } else {
            String::new()
        };
        let mut hints = Vec::new();
        if self.options.provider_switch_hint {
            hints.push("Tab toggle provider");
        }
        hints.push("↑↓ navigate");
        if searchable && !view.query.is_empty() {
            hints.push("Backspace clear");
        }
        hints.push("Enter select");
        if self.options.on_session_only_select.is_some() {
            hints.push("Alt+S session-only");
        }
        hints.push("Esc cancel");

        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            format!(
                "{}{}",
                current_theme().bold_fg(ColorToken::Primary, " Select a model"),
                title_suffix
            ),
            current_theme().fg(ColorToken::TextMuted, &format!(" {}", hints.join(" · "))),
        ];
        if let Some(warning) = &self.options.warning {
            for line in wrap_text_with_ansi(warning, width.saturating_sub(1).max(1)) {
                lines.push(current_theme().fg(ColorToken::Warning, &format!(" {line}")));
            }
        }
        lines.push(String::new());
        if searchable && !view.query.is_empty() {
            lines.push(format!(
                "{}{}",
                current_theme().fg(ColorToken::Primary, " Search: "),
                current_theme().fg(ColorToken::Text, view.query)
            ));
        }
        self.render_model_list(&view, width, &mut lines);
        self.render_match_indicator(&view, self.options.models.len(), &mut lines);
        lines.push(String::new());
        if let Some(selected) = self.list.selected() {
            let switchable = segments_for(&selected.model).len() > 1;
            let header = if switchable {
                " Thinking  (←/→ to switch)"
            } else {
                " Thinking"
            };
            lines.push(current_theme().fg(ColorToken::TextMuted, header));
            lines.push(self.render_thinking_control(selected));
        }
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }

    fn render_model_list(
        &self,
        view: &SearchableListView<'_, ModelChoice>,
        width: usize,
        lines: &mut Vec<String>,
    ) {
        if view.items.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "   No matches"));
            return;
        }
        let name_cap = 8.max(width / 2);
        let name_width = (view.page.start..view.page.end)
            .filter_map(|index| view.items.get(index))
            .map(|choice| visible_width(&choice.name))
            .max()
            .unwrap_or_default()
            .min(name_cap);
        for index in view.page.start..view.page.end {
            let Some(choice) = view.items.get(index) else {
                continue;
            };
            let selected = index == view.selected_index;
            let pointer = if selected { SELECT_POINTER } else { " " };
            let name = truncate_to_width(&choice.name, name_width, "…", false);
            let padding = " ".repeat(name_width.saturating_sub(visible_width(&name)));
            let mut line = current_theme().fg(
                if selected {
                    ColorToken::Primary
                } else {
                    ColorToken::TextDim
                },
                &format!("  {pointer} "),
            );
            let styled_name = if selected {
                current_theme().bold_fg(ColorToken::Primary, &name)
            } else {
                current_theme().fg(ColorToken::Text, &name)
            };
            line.push_str(&styled_name);
            line.push_str(&padding);
            line.push_str("  ");
            line.push_str(&current_theme().fg(ColorToken::TextMuted, &choice.provider));
            if choice.alias == self.options.current_value {
                line.push(' ');
                line.push_str(&current_theme().fg(ColorToken::Success, CURRENT_MARK));
            }
            lines.push(line);
        }
    }

    fn render_match_indicator(
        &self,
        view: &SearchableListView<'_, ModelChoice>,
        total_count: usize,
        lines: &mut Vec<String>,
    ) {
        if !view.query.is_empty() {
            lines.push(String::new());
            lines.push(current_theme().fg(
                ColorToken::TextMuted,
                &format!(" {} / {total_count}", view.items.len()),
            ));
        } else {
            let below = view.items.len().saturating_sub(view.page.end);
            if below > 0 {
                lines.push(String::new());
                lines.push(current_theme().fg(ColorToken::TextMuted, &format!(" ↓ {below} more")));
            }
        }
    }

    fn render_thinking_control(&self, choice: &ModelChoice) -> String {
        let segment = |label: &str, active: bool| {
            if active {
                current_theme().bold_fg(ColorToken::Primary, &format!("[ {label} ]"))
            } else {
                current_theme().fg(ColorToken::Text, &format!("  {label}  "))
            }
        };
        let unavailable = |label: &str| {
            current_theme().fg(ColorToken::TextMuted, &format!("  {label} (Unsupported)  "))
        };
        let efforts = efforts_of(&choice.model);
        let availability = thinking_availability(&choice.model);
        if efforts.is_empty() && availability == ThinkingAvailability::AlwaysOn {
            return format!("  {} {}", segment("On", true), unavailable("Off"));
        }
        if efforts.is_empty() && availability == ThinkingAvailability::Unsupported {
            return format!("  {} {}", unavailable("On"), segment("Off", true));
        }
        let active = self.effective_effort(choice);
        let rendered = segments_for(&choice.model)
            .iter()
            .map(|effort| segment(&effort_label(effort), effort == &active))
            .collect::<Vec<_>>();
        format!("  {}", rendered.join("  "))
    }
}

impl Component for ModelSelectorComponent {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn model(
        provider: &str,
        name: &str,
        capabilities: &[&str],
        efforts: &[&str],
        default_effort: Option<&str>,
    ) -> ModelAlias {
        ModelAlias {
            provider: provider.to_owned(),
            model: name.to_owned(),
            max_context_size: 128_000,
            max_output_size: None,
            capabilities: (!capabilities.is_empty()).then(|| {
                capabilities
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect()
            }),
            display_name: None,
            reasoning_key: None,
            protocol: None,
            adaptive_thinking: None,
            support_efforts: (!efforts.is_empty())
                .then(|| efforts.iter().map(|value| (*value).to_owned()).collect()),
            default_effort: default_effort.map(str::to_owned),
            beta_api: None,
            overrides: None,
        }
    }

    fn models() -> IndexMap<String, ModelAlias> {
        IndexMap::from([
            (
                "kimi".to_owned(),
                model(
                    "managed:kimi-code",
                    "kimi-k2",
                    &["thinking"],
                    &["low", "high", "max"],
                    Some("high"),
                ),
            ),
            (
                "plain".to_owned(),
                model("openai", "gpt-test", &[], &[], None),
            ),
            (
                "always".to_owned(),
                model("managed:team", "always", &["always_thinking"], &[], None),
            ),
        ])
    }

    #[test]
    fn derives_display_names_options_and_thinking_segments() {
        let entries = models();
        assert_eq!(provider_display_name("managed:kimi-code"), "Kimi Code");
        assert_eq!(provider_display_name("managed:team"), "team");
        assert_eq!(provider_display_name("openai"), "openai");
        assert_eq!(model_display_name("missing", None), "missing");
        assert_eq!(
            create_model_choice_options(&entries)
                .into_iter()
                .map(|option| option.label)
                .collect::<Vec<_>>(),
            ["kimi-k2 (Kimi Code)", "gpt-test (openai)", "always (team)"]
        );
        assert_eq!(
            segments_for(&entries["kimi"]),
            ["off", "low", "high", "max"]
        );
        assert_eq!(segments_for(&entries["plain"]), ["off"]);
        assert_eq!(segments_for(&entries["always"]), ["on"]);
    }

    #[test]
    fn filters_clears_query_then_cancels() {
        let cancellations = Arc::new(Mutex::new(0));
        let called = Arc::clone(&cancellations);
        let mut options = ModelSelectorOptions::new(
            models(),
            "kimi",
            ThinkingEffort::from("high"),
            |_| {},
            move || *called.lock().expect("cancellations") += 1,
        );
        options.searchable = true;
        let mut selector = ModelSelectorComponent::new(options);
        for character in "openai".chars() {
            selector.handle_input_event(&character.to_string());
        }
        assert_eq!(selector.selected_alias(), Some("plain"));
        selector.handle_input_event("\u{1b}");
        assert_eq!(selector.query(), "");
        assert_eq!(*cancellations.lock().expect("cancellations"), 0);
        selector.handle_input_event("\u{1b}");
        assert_eq!(*cancellations.lock().expect("cancellations"), 1);
    }

    #[test]
    fn toggles_efforts_and_commits_persistent_or_session_only_selection() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let persistent = Arc::clone(&events);
        let session = Arc::clone(&events);
        let options = ModelSelectorOptions::new(
            models(),
            "kimi",
            ThinkingEffort::from("high"),
            move |selection| {
                persistent.lock().expect("events").push(format!(
                    "select:{}:{}",
                    selection.alias,
                    selection.thinking.as_str()
                ));
            },
            || {},
        )
        .with_session_only_select(move |selection| {
            session.lock().expect("events").push(format!(
                "session:{}:{}",
                selection.alias,
                selection.thinking.as_str()
            ));
        });
        let mut selector = ModelSelectorComponent::new(options);
        selector.handle_input_event("\u{1b}[C");
        selector.handle_input_event("\r");
        selector.handle_input_event("\u{1b}s");
        assert_eq!(
            *events.lock().expect("events"),
            ["select:kimi:max", "session:kimi:max"]
        );
    }

    #[test]
    fn uses_model_defaults_and_locks_unsupported_or_always_on_controls() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let selected = Arc::clone(&events);
        let mut selector = ModelSelectorComponent::new(ModelSelectorOptions::new(
            models(),
            "kimi",
            ThinkingEffort::from("high"),
            move |selection| {
                selected
                    .lock()
                    .expect("events")
                    .push(selection.thinking.as_str().to_owned());
            },
            || {},
        ));
        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event("\u{1b}[C");
        selector.handle_input_event("\r");
        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event("\u{1b}[D");
        selector.handle_input_event("\r");
        assert_eq!(*events.lock().expect("events"), ["off", "on"]);
    }

    #[test]
    fn renders_aligned_models_search_warning_and_thinking_state_within_width() {
        let mut options = ModelSelectorOptions::new(
            models(),
            "kimi",
            ThinkingEffort::from("high"),
            |_| {},
            || {},
        );
        options.searchable = true;
        options.provider_switch_hint = true;
        options.warning = Some(
            "Switching models during a conversation may invalidate cached context.".to_owned(),
        );
        let mut selector = ModelSelectorComponent::new(options);
        let lines = selector.render(44);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Select a model")));
        assert!(
            plain
                .iter()
                .any(|line| line.contains("Tab toggle provider"))
        );
        assert!(plain.iter().any(|line| line.contains("← current")));
        assert!(plain.iter().any(|line| line.contains("[ High ]")));
        assert!(
            plain
                .iter()
                .filter(|line| line.contains("Switching") || line.contains("cached"))
                .count()
                >= 2
        );
        assert!(lines.iter().all(|line| visible_width(line) <= 44));
    }

    #[test]
    fn labels_empty_ascii_and_unicode_efforts() {
        assert_eq!(effort_label(""), "");
        assert_eq!(effort_label("high"), "High");
        assert_eq!(effort_label("élan"), "Élan");
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
