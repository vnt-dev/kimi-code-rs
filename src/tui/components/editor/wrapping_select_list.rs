use std::{any::Any, sync::Arc};

use crate::tui::components::{
    Component, ComponentRole,
    render::{truncate_to_width, visible_width, wrap_text_with_ansi},
};

const DEFAULT_PRIMARY_COLUMN_WIDTH: usize = 32;
const PRIMARY_COLUMN_GAP: usize = 2;
const MIN_DESCRIPTION_WIDTH: usize = 10;
const DESCRIPTION_MAX_LINES: usize = 2;
const ELLIPSIS: &str = "…";

type StyleFn = dyn Fn(&str) -> String + Send + Sync;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectItem {
    pub value: String,
    pub label: Option<String>,
    pub description: Option<String>,
}

impl SelectItem {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
            description: None,
        }
    }

    fn display_value(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.value)
    }
}

#[derive(Clone)]
pub struct SelectListTheme {
    pub selected_text: Arc<StyleFn>,
    pub description: Arc<StyleFn>,
    pub scroll_info: Arc<StyleFn>,
    pub no_match: Arc<StyleFn>,
}

impl SelectListTheme {
    pub fn identity() -> Self {
        let identity: Arc<StyleFn> = Arc::new(str::to_owned);
        Self {
            selected_text: Arc::clone(&identity),
            description: Arc::clone(&identity),
            scroll_info: Arc::clone(&identity),
            no_match: identity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SelectListLayout {
    pub min_primary_column_width: Option<usize>,
    pub max_primary_column_width: Option<usize>,
}

/// Slash-completion list that allows descriptions to occupy two rows.
///
/// Original: `src/tui/components/editor/wrapping-select-list.ts`,
/// `WrappingSelectList.render()`.
pub struct WrappingSelectList {
    items: Vec<SelectItem>,
    selected_index: usize,
    max_visible: usize,
    theme: SelectListTheme,
    layout: SelectListLayout,
}

impl WrappingSelectList {
    pub fn new(
        items: Vec<SelectItem>,
        max_visible: usize,
        theme: SelectListTheme,
        layout: SelectListLayout,
    ) -> Self {
        Self {
            items,
            selected_index: 0,
            max_visible: max_visible.max(1),
            theme,
            layout,
        }
    }

    pub fn set_items(&mut self, items: Vec<SelectItem>) {
        self.items = items;
        self.selected_index = self.selected_index.min(self.items.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&SelectItem> {
        self.items.get(self.selected_index)
    }

    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    pub fn set_selected_index(&mut self, index: usize) {
        self.selected_index = index.min(self.items.len().saturating_sub(1));
    }

    pub fn move_up(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        self.selected_index = self
            .selected_index
            .saturating_add(1)
            .min(self.items.len().saturating_sub(1));
    }

    fn render_item_lines(
        &self,
        item: &SelectItem,
        selected: bool,
        width: usize,
        primary_column_width: usize,
    ) -> Vec<String> {
        let prefix = if selected { "❯ " } else { "  " };
        let prefix_width = visible_width(prefix);
        let description = item
            .description
            .as_deref()
            .map(collapse_description)
            .filter(|description| !description.is_empty());

        if let Some(description) = description
            && width > 40
        {
            let effective_primary_column_width = primary_column_width
                .min(width.saturating_sub(prefix_width + 4))
                .max(1);
            let max_primary_width = effective_primary_column_width
                .saturating_sub(PRIMARY_COLUMN_GAP)
                .max(1);
            let truncated_value = truncate_plain_to_width(item.display_value(), max_primary_width);
            let spacing = " ".repeat(
                effective_primary_column_width
                    .saturating_sub(visible_width(&truncated_value))
                    .max(1),
            );
            let description_start = prefix_width + visible_width(&truncated_value) + spacing.len();
            let remaining_width = width.saturating_sub(description_start + 2);
            if remaining_width > MIN_DESCRIPTION_WIDTH {
                let description_lines = wrap_description(&description, remaining_width);
                let indent = " ".repeat(description_start);
                if selected {
                    return description_lines
                        .iter()
                        .enumerate()
                        .map(|(index, line)| {
                            let text = if index == 0 {
                                format!("{prefix}{truncated_value}{spacing}{line}")
                            } else {
                                format!("{indent}{line}")
                            };
                            (self.theme.selected_text)(&text)
                        })
                        .collect();
                }
                return description_lines
                    .iter()
                    .enumerate()
                    .map(|(index, line)| {
                        if index == 0 {
                            format!(
                                "{prefix}{truncated_value}{}",
                                (self.theme.description)(&format!("{spacing}{line}"))
                            )
                        } else {
                            (self.theme.description)(&format!("{indent}{line}"))
                        }
                    })
                    .collect();
            }
        }

        let max_width = width.saturating_sub(prefix_width + 2).max(1);
        let truncated_value = truncate_plain_to_width(item.display_value(), max_width);
        let text = format!("{prefix}{truncated_value}");
        vec![if selected {
            (self.theme.selected_text)(&text)
        } else {
            text
        }]
    }

    fn primary_column_width(&self) -> usize {
        let raw_min = self
            .layout
            .min_primary_column_width
            .or(self.layout.max_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let raw_max = self
            .layout
            .max_primary_column_width
            .or(self.layout.min_primary_column_width)
            .unwrap_or(DEFAULT_PRIMARY_COLUMN_WIDTH);
        let min = raw_min.min(raw_max).max(1);
        let max = raw_min.max(raw_max).max(1);
        let widest = self
            .items
            .iter()
            .map(|item| visible_width(item.display_value()) + PRIMARY_COLUMN_GAP)
            .max()
            .unwrap_or(0);
        widest.clamp(min, max)
    }
}

impl Component for WrappingSelectList {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.items.is_empty() {
            return vec![(self.theme.no_match)("  No matching commands")];
        }

        let primary_column_width = self.primary_column_width();
        let half = self.max_visible / 2;
        let last_start = self.items.len().saturating_sub(self.max_visible);
        let start = self.selected_index.saturating_sub(half).min(last_start);
        let end = (start + self.max_visible).min(self.items.len());
        let mut lines = Vec::new();
        for index in start..end {
            lines.extend(self.render_item_lines(
                &self.items[index],
                index == self.selected_index,
                width,
                primary_column_width,
            ));
        }

        if start > 0 || end < self.items.len() {
            let text = format!("  ({}/{})", self.selected_index + 1, self.items.len());
            lines.push((self.theme.scroll_info)(&truncate_plain_to_width(
                &text,
                width.saturating_sub(2),
            )));
        }
        lines
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn collapse_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_plain_to_width(text: &str, max_width: usize) -> String {
    truncate_to_width(text, max_width, "", false)
        .trim_end_matches("\u{1b}[0m")
        .to_owned()
}

fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let wrapped = wrap_text_with_ansi(text, width);
    if wrapped.len() <= DESCRIPTION_MAX_LINES {
        return wrapped;
    }
    let mut kept = wrapped[..DESCRIPTION_MAX_LINES - 1].to_vec();
    let rest = wrapped[DESCRIPTION_MAX_LINES - 1..].join(" ");
    let clipped = truncate_plain_to_width(&rest, width.saturating_sub(visible_width(ELLIPSIS)));
    kept.push(format!("{}{ELLIPSIS}", clipped.trim_end()));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker_theme() -> SelectListTheme {
        SelectListTheme {
            selected_text: Arc::new(|text| format!("[S]{text}")),
            description: Arc::new(|text| format!("[D]{text}")),
            scroll_info: Arc::new(|text| format!("[I]{text}")),
            no_match: Arc::new(|text| format!("[N]{text}")),
        }
    }

    fn item(value: &str, description: &str) -> SelectItem {
        SelectItem {
            value: value.to_owned(),
            label: Some(value.to_owned()),
            description: Some(description.to_owned()),
        }
    }

    fn list(items: Vec<SelectItem>, max_visible: usize) -> WrappingSelectList {
        WrappingSelectList::new(
            items,
            max_visible,
            marker_theme(),
            SelectListLayout {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(32),
            },
        )
    }

    #[test]
    fn renders_short_descriptions_and_narrow_primary_only_rows() {
        let items = vec![
            item("goal", "First command"),
            item("init", "Second command"),
        ];
        let mut wide = list(items.clone(), 5);
        assert_eq!(
            wide.render(80),
            [
                "[S]❯ goal        First command",
                "  init[D]        Second command"
            ]
        );

        let mut narrow = list(items, 5);
        assert_eq!(narrow.render(40), ["[S]❯ goal", "  init"]);
    }

    #[test]
    fn wraps_to_two_lines_and_ellipsizes_overflow() {
        let mut wrapped = list(
            vec![
                item("goal", "First command"),
                item(
                    "init",
                    "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt",
                ),
            ],
            5,
        );
        let lines = wrapped.render(80);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1],
            "  init[D]        lorem ipsum dolor sit amet consectetur adipiscing elit sed do"
        );
        assert_eq!(lines[2], "[D]              eiusmod tempor incididunt");

        let description = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(4);
        let mut ellipsized = list(
            vec![item("goal", &description), item("init", "Second command")],
            5,
        );
        let lines = ellipsized.render(80);
        assert!(lines[1].starts_with("[S]              "));
        assert!(lines[1].ends_with(ELLIPSIS));
        assert_eq!(lines[2], "  init[D]        Second command");
    }

    #[test]
    fn selected_style_paints_every_wrapped_line() {
        let description = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(4);
        let mut component = list(
            vec![item("goal", &description), item("init", "Second command")],
            5,
        );
        let lines = component.render(80);
        assert!(lines[0].starts_with("[S]❯ goal"));
        assert!(lines[1].starts_with("[S]              "));
    }

    #[test]
    fn centers_selection_and_keeps_scroll_indicator() {
        let items = (0..7)
            .map(|index| item(&format!("cmd{index}"), "Short"))
            .collect();
        let mut component = list(items, 5);
        let lines = component.render(80);
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[5], "[I]  (1/7)");

        component.set_selected_index(6);
        let lines = component.render(80);
        assert!(lines.iter().any(|line| line.starts_with("[S]❯ cmd6")));
        assert_eq!(lines.last().map(String::as_str), Some("[I]  (7/7)"));
    }

    #[test]
    fn handles_empty_items_newlines_long_names_and_cjk_width() {
        let mut empty = list(Vec::new(), 5);
        assert_eq!(empty.render(80), ["[N]  No matching commands"]);

        let mut component = WrappingSelectList::new(
            vec![
                item(
                    "skill:verification-before-completion",
                    "Use when\nabout to claim work is complete",
                ),
                item("skill:lark-calendar", &"管理飞书日历的技能描述".repeat(8)),
            ],
            5,
            SelectListTheme::identity(),
            SelectListLayout {
                min_primary_column_width: Some(12),
                max_primary_column_width: Some(32),
            },
        );
        let lines = component.render(80);
        assert!(lines.iter().all(|line| visible_width(line) <= 80));
        assert!(lines.iter().all(|line| !line.contains("\u{1b}[0m")));
        assert!(lines.join(" ").contains("Use when about to claim"));
    }
}
