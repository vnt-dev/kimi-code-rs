use std::{any::Any, collections::HashMap};

use crate::tui::{
    commands::experimental_flags::{ExperimentalFeatureState, ExperimentalFlagSource},
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::{printable_key::printable_char, searchable_list::SearchableList},
};

const ELLIPSIS: &str = "…";
const SELECT_POINTER: &str = "❯";

type ApplyCallback = dyn FnMut(Vec<ExperimentalFeatureDraftChange>) + Send;
type CancelCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentalFeatureDraftChange {
    pub id: String,
    pub enabled: bool,
}

pub struct ExperimentsSelectorOptions {
    pub features: Vec<ExperimentalFeatureState>,
    on_apply: Box<ApplyCallback>,
    on_cancel: Box<CancelCallback>,
}

impl ExperimentsSelectorOptions {
    pub fn new<A, C>(features: Vec<ExperimentalFeatureState>, on_apply: A, on_cancel: C) -> Self
    where
        A: FnMut(Vec<ExperimentalFeatureDraftChange>) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            features,
            on_apply: Box::new(on_apply),
            on_cancel: Box::new(on_cancel),
        }
    }
}

/// Searchable experimental-feature editor with apply-on-enter draft state.
///
/// Original: `experiments-selector.ts`, `ExperimentsSelectorComponent`.
pub struct ExperimentsSelectorComponent {
    pub focused: bool,
    options: ExperimentsSelectorOptions,
    list: SearchableList<ExperimentalFeatureState>,
    draft: HashMap<String, bool>,
}

impl ExperimentsSelectorComponent {
    pub fn new(options: ExperimentsSelectorOptions) -> Self {
        let list = SearchableList::new(
            options.features.clone(),
            |feature| format!("{} {} {}", feature.title, feature.id, feature.description),
            None,
            None,
            true,
        );
        Self {
            focused: false,
            options,
            list,
            draft: HashMap::new(),
        }
    }

    /// Original: `ExperimentsSelectorComponent.handleInput()`.
    pub fn handle_input_event(&mut self, data: &str) {
        if matches_editor_key(data, EditorKey::Escape) {
            if self.list.clear_query() {
                return;
            }
            (self.options.on_cancel)();
            return;
        }
        if matches_editor_key(data, EditorKey::Enter) {
            let changes = self.draft_changes();
            if !changes.is_empty() {
                (self.options.on_apply)(changes);
            }
            return;
        }
        if printable_char(data) == " " {
            if let Some(selected) = self.list.selected().cloned() {
                self.toggle_draft(&selected);
            }
            return;
        }
        self.list.handle_key(data);
    }

    /// Original: `ExperimentsSelectorComponent.render()`.
    pub fn render_selector(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let view = self.list.view();
        let title_suffix = if view.query.is_empty() {
            theme.fg(ColorToken::TextMuted, "  (type to search)")
        } else {
            String::new()
        };
        let mut hint_parts = vec!["↑↓ navigate"];
        if view.page.page_count > 1 {
            hint_parts.push("PgUp/PgDn page");
        }
        hint_parts.extend(["Space toggle", "Enter apply", "Esc cancel"]);
        if !view.query.is_empty() {
            hint_parts.push("Backspace clear");
        }

        let mut lines = vec![
            theme.fg(ColorToken::Primary, &"─".repeat(width)),
            format!(
                "{}{}",
                theme.bold_fg(ColorToken::Primary, " Experimental features"),
                title_suffix
            ),
            theme.fg(
                ColorToken::TextMuted,
                &format!(" {}", hint_parts.join(" · ")),
            ),
            String::new(),
        ];
        if !view.query.is_empty() {
            lines.push(format!(
                "{}{}",
                theme.fg(ColorToken::Primary, " Search: "),
                theme.fg(ColorToken::Text, view.query)
            ));
        }
        if view.items.is_empty() {
            lines.push(theme.fg(ColorToken::TextMuted, "   No matches"));
        }
        for index in view.page.start..view.page.end {
            let feature = view.items[index];
            lines.extend(self.render_feature(feature, index == view.selected_index, width));
        }

        lines.push(String::new());
        if !view.query.is_empty() {
            lines.push(theme.fg(
                ColorToken::TextMuted,
                &format!(" {} / {}", view.items.len(), self.options.features.len()),
            ));
        } else if view.page.end < view.items.len() {
            lines.push(theme.fg(
                ColorToken::TextMuted,
                &format!(" ↓ {} more", view.items.len() - view.page.end),
            ));
        }
        lines.push(self.render_apply_button());
        lines.push(theme.fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, ELLIPSIS, false))
            .collect()
    }

    /// Original: `ExperimentsSelectorComponent.toggleDraft()`.
    fn toggle_draft(&mut self, feature: &ExperimentalFeatureState) {
        if is_locked(feature) {
            return;
        }
        let enabled = !self.effective_enabled(feature);
        if enabled == feature.enabled {
            self.draft.remove(&feature.id);
        } else {
            self.draft.insert(feature.id.clone(), enabled);
        }
    }

    /// Original: `ExperimentsSelectorComponent.effectiveEnabled()`.
    fn effective_enabled(&self, feature: &ExperimentalFeatureState) -> bool {
        self.draft
            .get(&feature.id)
            .copied()
            .unwrap_or(feature.enabled)
    }

    /// Original: `ExperimentsSelectorComponent.isDraftChanged()`.
    fn is_draft_changed(&self, feature: &ExperimentalFeatureState) -> bool {
        self.effective_enabled(feature) != feature.enabled
    }

    /// Original: `ExperimentsSelectorComponent.draftChanges()`.
    pub fn draft_changes(&self) -> Vec<ExperimentalFeatureDraftChange> {
        self.options
            .features
            .iter()
            .filter(|feature| self.is_draft_changed(feature))
            .map(|feature| ExperimentalFeatureDraftChange {
                id: feature.id.clone(),
                enabled: self.effective_enabled(feature),
            })
            .collect()
    }

    /// Original: `ExperimentsSelectorComponent.renderApplyButton()`.
    fn render_apply_button(&self) -> String {
        let count = self.draft_changes().len();
        let label = "[ Apply changes and reload ]";
        let summary = if count == 0 {
            "no changes".to_owned()
        } else if count == 1 {
            "1 change".to_owned()
        } else {
            format!("{count} changes")
        };
        let theme = current_theme();
        let button = if count == 0 {
            theme.fg(ColorToken::TextDim, label)
        } else {
            theme.bold_fg(ColorToken::Primary, label)
        };
        let summary = if count == 0 {
            theme.fg(ColorToken::TextMuted, &summary)
        } else {
            theme.fg(ColorToken::Success, &summary)
        };
        format!(" {button}  {summary}")
    }

    /// Original: `ExperimentsSelectorComponent.renderFeature()`.
    fn render_feature(
        &self,
        feature: &ExperimentalFeatureState,
        selected: bool,
        width: usize,
    ) -> Vec<String> {
        let theme = current_theme();
        let pointer = if selected { SELECT_POINTER } else { " " };
        let prefix = theme.fg(
            if selected {
                ColorToken::Primary
            } else {
                ColorToken::TextDim
            },
            &format!("  {pointer} "),
        );
        let label = if selected {
            theme.bold_fg(ColorToken::Primary, &feature.title)
        } else {
            theme.fg(ColorToken::Text, &feature.title)
        };
        let enabled = self.effective_enabled(feature);
        let status = if enabled { "enabled" } else { "disabled" };
        let status = theme.fg(
            if enabled {
                ColorToken::Success
            } else {
                ColorToken::TextDim
            },
            status,
        );
        let mut detail = feature_detail(feature);
        if self.is_draft_changed(feature) {
            detail.push_str(" · modified");
        }
        let mut lines = vec![
            format!("{prefix}{label}  {status}"),
            theme.fg(ColorToken::TextMuted, &format!("    {detail}")),
        ];
        for line in wrap_text(&feature.description, width.saturating_sub(4).max(1)) {
            lines.push(theme.fg(ColorToken::TextMuted, &format!("    {line}")));
        }
        lines
    }
}

impl Component for ExperimentsSelectorComponent {
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

/// Original: `experiments-selector.ts`, `isLocked()`.
fn is_locked(feature: &ExperimentalFeatureState) -> bool {
    matches!(
        feature.source,
        ExperimentalFlagSource::Env | ExperimentalFlagSource::MasterEnv
    )
}

/// Original: `experiments-selector.ts`, `featureDetail()`.
fn feature_detail(feature: &ExperimentalFeatureState) -> String {
    let source = source_label(feature);
    if is_locked(feature) {
        format!("id {} · {source}", feature.id)
    } else {
        format!("id {} · {source} · {}", feature.id, feature.env)
    }
}

/// Original: `experiments-selector.ts`, `sourceLabel()`.
fn source_label(feature: &ExperimentalFeatureState) -> String {
    match feature.source {
        ExperimentalFlagSource::MasterEnv => "locked by KIMI_CODE_EXPERIMENTAL_FLAG".to_owned(),
        ExperimentalFlagSource::Env => format!("locked by {}", feature.env),
        ExperimentalFlagSource::Config => "config".to_owned(),
        ExperimentalFlagSource::Default => "default".to_owned(),
    }
}

/// Original: `experiments-selector.ts`, `wrapText()`.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
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
            continue;
        }
        if !current.is_empty() {
            lines.push(current);
        }
        current = if visible_width(word) <= width {
            word.to_owned()
        } else {
            truncate_to_width(word, width, ELLIPSIS, false)
        };
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::commands::experimental_flags::FlagSurface;

    use super::*;

    fn feature(
        id: &str,
        enabled: bool,
        source: ExperimentalFlagSource,
    ) -> ExperimentalFeatureState {
        ExperimentalFeatureState {
            id: id.to_owned(),
            title: format!("Feature {id}"),
            description: format!("Description for experimental feature {id}"),
            surface: FlagSurface::Both,
            env: format!("KIMI_CODE_{}", id.to_ascii_uppercase()),
            default_enabled: false,
            enabled,
            source,
            config_value: None,
        }
    }

    type Changes = Arc<Mutex<Vec<Vec<ExperimentalFeatureDraftChange>>>>;

    fn selector(
        features: Vec<ExperimentalFeatureState>,
    ) -> (ExperimentsSelectorComponent, Changes, Arc<Mutex<usize>>) {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let cancelled = Arc::new(Mutex::new(0));
        let applied_callback = Arc::clone(&applied);
        let cancelled_callback = Arc::clone(&cancelled);
        let selector = ExperimentsSelectorComponent::new(ExperimentsSelectorOptions::new(
            features,
            move |changes| {
                applied_callback
                    .lock()
                    .expect("applied changes")
                    .push(changes)
            },
            move || *cancelled_callback.lock().expect("cancel count") += 1,
        ));
        (selector, applied, cancelled)
    }

    fn plain(lines: &[String]) -> String {
        let ansi = regex::Regex::new("\\x1b\\[[0-9;]*m").expect("ANSI regex");
        ansi.replace_all(&lines.join("\n"), "").into_owned()
    }

    #[test]
    fn toggles_reverts_and_applies_changes_in_original_feature_order() {
        let (mut selector, applied, _) = selector(vec![
            feature("a", false, ExperimentalFlagSource::Default),
            feature("b", true, ExperimentalFlagSource::Config),
        ]);
        selector.handle_input_event(" ");
        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event(" ");
        assert_eq!(
            selector.draft_changes(),
            [
                ExperimentalFeatureDraftChange {
                    id: "a".to_owned(),
                    enabled: true
                },
                ExperimentalFeatureDraftChange {
                    id: "b".to_owned(),
                    enabled: false
                }
            ]
        );
        selector.handle_input_event("\r");
        assert_eq!(applied.lock().expect("applied changes").len(), 1);

        selector.handle_input_event(" ");
        assert_eq!(selector.draft_changes().len(), 1);
    }

    #[test]
    fn locked_features_ignore_space_and_empty_enter_does_not_apply() {
        for source in [
            ExperimentalFlagSource::Env,
            ExperimentalFlagSource::MasterEnv,
        ] {
            let (mut selector, applied, _) = selector(vec![feature("a", false, source)]);
            selector.handle_input_event(" ");
            selector.handle_input_event("\r");
            assert!(selector.draft_changes().is_empty());
            assert!(applied.lock().expect("applied changes").is_empty());
        }
    }

    #[test]
    fn escape_clears_query_before_cancelling() {
        let (mut selector, _, cancelled) = selector(vec![feature(
            "searchable",
            false,
            ExperimentalFlagSource::Default,
        )]);
        selector.handle_input_event("z");
        assert!(plain(&selector.render_selector(60)).contains("No matches"));
        selector.handle_input_event("\u{1b}");
        assert_eq!(*cancelled.lock().expect("cancel count"), 0);
        selector.handle_input_event("\u{1b}");
        assert_eq!(*cancelled.lock().expect("cancel count"), 1);
    }

    #[test]
    fn renders_modified_locked_search_paging_and_bounded_descriptions() {
        let features = (0..10)
            .map(|index| {
                feature(
                    &format!("f{index}"),
                    index % 2 == 0,
                    if index == 0 {
                        ExperimentalFlagSource::Env
                    } else {
                        ExperimentalFlagSource::Default
                    },
                )
            })
            .collect();
        let (mut selector, _, _) = selector(features);
        let initial = selector.render_selector(48);
        let initial_plain = plain(&initial);
        assert!(initial_plain.contains("type to search"));
        assert!(initial_plain.contains("↓ 2 more"));
        assert!(initial_plain.contains("locked by KIMI_CODE_F0"));
        assert!(initial.iter().all(|line| visible_width(line) <= 48));

        selector.handle_input_event("\u{1b}[B");
        selector.handle_input_event(" ");
        let modified = plain(&selector.render_selector(48));
        assert!(modified.contains("modified"));
        assert!(modified.contains("1 change"));
    }

    #[test]
    fn wraps_words_and_truncates_a_single_long_token() {
        assert_eq!(wrap_text("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap_text("abcdefgh", 4), ["abc…"]);
        assert!(wrap_text("   ", 4).is_empty());
    }
}
