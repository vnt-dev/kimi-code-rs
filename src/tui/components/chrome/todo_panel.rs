use std::{any::Any, collections::HashSet};

pub use crate::tui::utils::event_payload::{TodoItem, TodoItemStatus};
use crate::tui::{
    components::{Component, ComponentRole, render::truncate_to_width},
    theme::{ColorToken, current_theme},
};

const MAX_VISIBLE: usize = 5;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HiddenTodoCounts {
    pub done: usize,
    pub in_progress: usize,
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleTodos {
    pub rows: Vec<TodoItem>,
    pub hidden: usize,
    pub hidden_counts: HiddenTodoCounts,
}

/// Chooses the current work, earliest pending work, and most recent completed
/// work while retaining the model-provided ordering.
///
/// Original: `todo-panel.ts`, `selectVisibleTodos()`.
pub fn select_visible_todos(todos: &[TodoItem]) -> VisibleTodos {
    if todos.len() <= MAX_VISIBLE {
        return VisibleTodos {
            rows: todos.to_vec(),
            hidden: 0,
            hidden_counts: HiddenTodoCounts::default(),
        };
    }

    let mut in_progress = Vec::new();
    let mut pending = Vec::new();
    let mut done = Vec::new();
    for (index, todo) in todos.iter().enumerate() {
        match todo.status {
            TodoItemStatus::InProgress => in_progress.push(index),
            TodoItemStatus::Pending => pending.push(index),
            TodoItemStatus::Done => done.push(index),
        }
    }

    let mut picked = HashSet::new();
    picked.extend(in_progress.into_iter().take(MAX_VISIBLE));

    if picked.len() < MAX_VISIBLE {
        let done_candidates = done.into_iter().rev().collect::<Vec<_>>();
        let remaining = MAX_VISIBLE - picked.len();
        let (done_count, pending_count) = if done_candidates.is_empty() {
            (0, remaining.min(pending.len()))
        } else if pending.is_empty() {
            (remaining.min(done_candidates.len()), 0)
        } else {
            let pending_count = (remaining - 1).min(pending.len());
            let done_count = if pending_count < remaining - 1 {
                done_candidates.len().min(remaining - pending_count)
            } else {
                1
            };
            (done_count, pending_count)
        };
        picked.extend(done_candidates.into_iter().take(done_count));
        picked.extend(pending.into_iter().take(pending_count));
    }

    let mut sorted_indices = picked.iter().copied().collect::<Vec<_>>();
    sorted_indices.sort_unstable();
    let mut hidden_counts = HiddenTodoCounts::default();
    for (index, todo) in todos.iter().enumerate() {
        if !picked.contains(&index) {
            match todo.status {
                TodoItemStatus::Done => hidden_counts.done += 1,
                TodoItemStatus::InProgress => hidden_counts.in_progress += 1,
                TodoItemStatus::Pending => hidden_counts.pending += 1,
            }
        }
    }

    VisibleTodos {
        rows: sorted_indices
            .into_iter()
            .map(|index| todos[index].clone())
            .collect(),
        hidden: todos.len() - picked.len(),
        hidden_counts,
    }
}

pub fn format_hidden_counts(counts: HiddenTodoCounts) -> String {
    let mut labels = Vec::new();
    if counts.done > 0 {
        labels.push(format!("{} done", counts.done));
    }
    if counts.in_progress > 0 {
        labels.push(format!("{} in progress", counts.in_progress));
    }
    if counts.pending > 0 {
        labels.push(format!("{} pending", counts.pending));
    }
    labels.join(" · ")
}

/// Live-updating TODO list mounted before the queue and editor.
///
/// Original: `src/tui/components/chrome/todo-panel.ts`,
/// `TodoPanelComponent`.
#[derive(Debug, Default)]
pub struct TodoPanelComponent {
    todos: Vec<TodoItem>,
    expanded: bool,
}

impl TodoPanelComponent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_todos(&mut self, todos: &[TodoItem]) {
        self.todos = todos.to_vec();
    }

    pub fn todos(&self) -> &[TodoItem] {
        &self.todos
    }

    pub fn clear(&mut self) {
        self.todos.clear();
        self.expanded = false;
    }

    pub fn is_empty(&self) -> bool {
        self.todos.is_empty()
    }

    pub fn has_overflow(&self) -> bool {
        self.todos.len() > MAX_VISIBLE
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }
}

impl Component for TodoPanelComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        if self.todos.is_empty() {
            return Vec::new();
        }
        let theme = current_theme();
        let mut lines = vec![
            theme.fg(ColorToken::Border, &"─".repeat(width)),
            theme.bold_fg(ColorToken::Primary, "  Todo"),
        ];

        if self.expanded {
            lines.extend(self.todos.iter().map(render_row));
            if self.todos.len() > MAX_VISIBLE {
                lines.push(theme.fg(
                    ColorToken::TextDim,
                    &format!("  all {} items · ctrl+t to collapse", self.todos.len()),
                ));
            }
        } else {
            let visible = select_visible_todos(&self.todos);
            lines.extend(visible.rows.iter().map(render_row));
            if visible.hidden > 0 {
                let distribution = format_hidden_counts(visible.hidden_counts);
                let suffix = if distribution.is_empty() {
                    String::new()
                } else {
                    format!(" ({distribution})")
                };
                lines.push(theme.fg(
                    ColorToken::TextDim,
                    &format!("  … +{} more{} · ctrl+t to expand", visible.hidden, suffix),
                ));
            }
        }

        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, ELLIPSIS, false))
            .collect()
    }

    fn invalidate(&mut self) {}

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn render_row(todo: &TodoItem) -> String {
    let theme = current_theme();
    let marker = match todo.status {
        TodoItemStatus::InProgress => theme.bold_fg(ColorToken::Primary, "●"),
        TodoItemStatus::Done => theme.fg(ColorToken::Success, "✓"),
        TodoItemStatus::Pending => theme.fg(ColorToken::TextDim, "○"),
    };
    let title = match todo.status {
        TodoItemStatus::InProgress => theme.bold_fg(ColorToken::Text, &todo.title),
        TodoItemStatus::Done => theme.strikethrough_fg(ColorToken::TextDim, &todo.title),
        TodoItemStatus::Pending => theme.fg(ColorToken::Text, &todo.title),
    };
    format!("  {marker} {title}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    fn todo(title: &str, status: TodoItemStatus) -> TodoItem {
        TodoItem {
            title: title.to_owned(),
            status,
        }
    }

    fn titles(visible: &VisibleTodos) -> Vec<&str> {
        visible
            .rows
            .iter()
            .map(|todo| todo.title.as_str())
            .collect()
    }

    #[test]
    fn empty_set_replace_clear_and_defensive_copy() {
        let mut panel = TodoPanelComponent::new();
        assert!(panel.render(80).is_empty());
        assert!(panel.is_empty());

        let mut source = vec![todo("old", TodoItemStatus::Pending)];
        panel.set_todos(&source);
        source[0] = todo("hacked", TodoItemStatus::Done);
        let output = panel.render(80).join("\n");
        assert!(output.contains("old"));
        assert!(!output.contains("hacked"));

        panel.set_todos(&[todo("new", TodoItemStatus::InProgress)]);
        let output = panel.render(80).join("\n");
        assert!(output.contains("new"));
        assert!(!output.contains("old"));
        panel.clear();
        assert!(panel.is_empty());
    }

    #[test]
    fn renders_status_rows_and_truncates_to_width() {
        let mut panel = TodoPanelComponent::new();
        panel.set_todos(&[
            todo("Investigate parser", TodoItemStatus::Done),
            todo("Add tests", TodoItemStatus::InProgress),
            todo("Open PR", TodoItemStatus::Pending),
        ]);
        let lines = panel.render(18);
        let output = lines.join("\n");
        assert!(output.contains("Todo"));
        assert!(output.contains('✓'));
        assert!(output.contains('●'));
        assert!(output.contains('○'));
        assert!(lines.iter().all(|line| visible_width(line) <= 18));
    }

    #[test]
    fn collapsed_and_expanded_footers_and_state_match() {
        let todos = (0..7)
            .map(|index| todo(&format!("t{index}"), TodoItemStatus::Pending))
            .collect::<Vec<_>>();
        let mut panel = TodoPanelComponent::new();
        panel.set_todos(&todos);
        assert!(panel.has_overflow());
        let collapsed = panel.render(80).join("\n");
        assert!(collapsed.contains("+2 more"));
        assert!(collapsed.contains("ctrl+t to expand"));
        assert!(!collapsed.contains("t6"));

        panel.toggle_expanded();
        let expanded = panel.render(80).join("\n");
        assert!(expanded.contains("t6"));
        assert!(expanded.contains("all 7 items · ctrl+t to collapse"));
        panel.set_todos(&todos);
        assert!(panel.render(80).join("\n").contains("ctrl+t to collapse"));

        panel.clear();
        panel.set_todos(&todos);
        assert!(panel.render(80).join("\n").contains("ctrl+t to expand"));
    }

    #[test]
    fn selection_balances_recent_done_current_and_earliest_pending() {
        let visible = select_visible_todos(&[
            todo("d1", TodoItemStatus::Done),
            todo("d2", TodoItemStatus::Done),
            todo("d3", TodoItemStatus::Done),
            todo("ip", TodoItemStatus::InProgress),
            todo("p1", TodoItemStatus::Pending),
            todo("p2", TodoItemStatus::Pending),
            todo("p3", TodoItemStatus::Pending),
            todo("p4", TodoItemStatus::Pending),
        ]);
        assert_eq!(titles(&visible), ["d3", "ip", "p1", "p2", "p3"]);
        assert_eq!(visible.hidden, 3);

        let interleaved = select_visible_todos(&[
            todo("p0", TodoItemStatus::Pending),
            todo("d0", TodoItemStatus::Done),
            todo("p1", TodoItemStatus::Pending),
            todo("d1", TodoItemStatus::Done),
            todo("p2", TodoItemStatus::Pending),
            todo("d2", TodoItemStatus::Done),
            todo("p3", TodoItemStatus::Pending),
        ]);
        assert_eq!(interleaved.rows.len(), MAX_VISIBLE);
        assert_eq!(interleaved.hidden, 2);
        assert_eq!(
            interleaved
                .rows
                .iter()
                .filter(|todo| todo.status == TodoItemStatus::Done)
                .count(),
            1
        );
    }

    #[test]
    fn selection_handles_single_status_and_in_progress_cap() {
        let pending = (0..8)
            .map(|index| todo(&format!("p{index}"), TodoItemStatus::Pending))
            .collect::<Vec<_>>();
        assert_eq!(
            titles(&select_visible_todos(&pending)),
            ["p0", "p1", "p2", "p3", "p4"]
        );

        let done = (0..8)
            .map(|index| todo(&format!("d{index}"), TodoItemStatus::Done))
            .collect::<Vec<_>>();
        assert_eq!(
            titles(&select_visible_todos(&done)),
            ["d3", "d4", "d5", "d6", "d7"]
        );

        let in_progress = (0..7)
            .map(|index| todo(&format!("ip{index}"), TodoItemStatus::InProgress))
            .collect::<Vec<_>>();
        assert_eq!(
            titles(&select_visible_todos(&in_progress)),
            ["ip0", "ip1", "ip2", "ip3", "ip4"]
        );
    }

    #[test]
    fn hidden_counts_and_formatting_preserve_status_order() {
        let todos = (0..6)
            .map(|index| todo(&format!("ip{index}"), TodoItemStatus::InProgress))
            .chain((0..3).map(|index| todo(&format!("d{index}"), TodoItemStatus::Done)))
            .chain((0..3).map(|index| todo(&format!("p{index}"), TodoItemStatus::Pending)))
            .collect::<Vec<_>>();
        let visible = select_visible_todos(&todos);
        assert_eq!(visible.hidden, 7);
        assert_eq!(
            visible.hidden_counts,
            HiddenTodoCounts {
                done: 3,
                in_progress: 1,
                pending: 3,
            }
        );
        assert_eq!(
            format_hidden_counts(visible.hidden_counts),
            "3 done · 1 in progress · 3 pending"
        );
        assert_eq!(
            format_hidden_counts(HiddenTodoCounts {
                done: 0,
                in_progress: 2,
                pending: 3,
            }),
            "2 in progress · 3 pending"
        );
        assert_eq!(format_hidden_counts(HiddenTodoCounts::default()), "");
    }
}
