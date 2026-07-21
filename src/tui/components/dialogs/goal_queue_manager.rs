use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    goal_queue_store::{GoalQueueMoveDirection, GoalQueueSnapshot, UpcomingGoal},
    keys::{EditorKey, matches_editor_key},
    theme::{ColorToken, current_theme},
    utils::{printable_key::printable_char, searchable_list::SearchableList},
};

const SELECT_POINTER: &str = "❯";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalQueueManagerAction {
    Move {
        goal_id: String,
        direction: GoalQueueMoveDirection,
    },
    Edit {
        goal_id: String,
    },
    Delete {
        goal_id: String,
    },
}

type ActionCallback = dyn FnMut(GoalQueueManagerAction) + Send;
type CancelCallback = dyn FnMut() + Send;

pub struct GoalQueueManagerOptions {
    pub goals: Vec<UpcomingGoal>,
    pub selected_goal_id: Option<String>,
    pub page_size: Option<isize>,
    on_action: Box<ActionCallback>,
    on_cancel: Box<CancelCallback>,
}

impl GoalQueueManagerOptions {
    pub fn new<A, C>(goals: Vec<UpcomingGoal>, on_action: A, on_cancel: C) -> Self
    where
        A: FnMut(GoalQueueManagerAction) + Send + 'static,
        C: FnMut() + Send + 'static,
    {
        Self {
            goals,
            selected_goal_id: None,
            page_size: None,
            on_action: Box::new(on_action),
            on_cancel: Box::new(on_cancel),
        }
    }
}

/// Upcoming-goal list and reorder state machine.
///
/// Original: `goal-queue-manager.ts`, `GoalQueueManagerComponent`.
/// Async persistence is dispatched through `on_action`; the host returns its
/// result through [`Self::complete_action`] so input remains locked in flight.
pub struct GoalQueueManagerComponent {
    pub focused: bool,
    page_size: Option<isize>,
    goals: Vec<UpcomingGoal>,
    list: SearchableList<UpcomingGoal>,
    moving_goal_id: Option<String>,
    pending_action: Option<GoalQueueManagerAction>,
    on_action: Box<ActionCallback>,
    on_cancel: Box<CancelCallback>,
}

impl GoalQueueManagerComponent {
    pub fn new(options: GoalQueueManagerOptions) -> Self {
        let list = create_list(
            &options.goals,
            options.selected_goal_id.as_deref(),
            options.page_size,
        );
        Self {
            focused: false,
            page_size: options.page_size,
            goals: options.goals,
            list,
            moving_goal_id: None,
            pending_action: None,
            on_action: options.on_action,
            on_cancel: options.on_cancel,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.pending_action.is_some()
    }
    pub fn selected_goal_id(&self) -> Option<&str> {
        self.list.selected().map(|goal| goal.id.as_str())
    }

    /// Completes a previously dispatched move/delete action.
    pub fn complete_action(&mut self, snapshot: Option<GoalQueueSnapshot>) {
        let Some(action) = self.pending_action.take() else {
            return;
        };
        if let Some(snapshot) = snapshot {
            let selected = match &action {
                GoalQueueManagerAction::Delete { .. } => None,
                GoalQueueManagerAction::Move { goal_id, .. } => Some(goal_id.as_str()),
                GoalQueueManagerAction::Edit { .. } => None,
            };
            self.goals = snapshot.goals;
            if self
                .moving_goal_id
                .as_ref()
                .is_some_and(|id| !self.goals.iter().any(|goal| &goal.id == id))
            {
                self.moving_goal_id = None;
            }
            self.list = create_list(
                &self.goals,
                selected.or(self.moving_goal_id.as_deref()),
                self.page_size,
            );
        }
    }

    pub fn handle_input_event(&mut self, data: &str) {
        if self.is_busy() {
            return;
        }
        if matches_editor_key(data, EditorKey::Escape) {
            (self.on_cancel)();
            return;
        }
        let selected = self.list.selected().map(|goal| goal.id.clone());
        let decoded = printable_char(data);
        if decoded == " " {
            self.moving_goal_id = if self.moving_goal_id == selected {
                None
            } else {
                selected
            };
            return;
        }
        if matches!(decoded.as_str(), "e" | "E") {
            if let Some(goal_id) = selected {
                (self.on_action)(GoalQueueManagerAction::Edit { goal_id });
            }
            return;
        }
        if matches!(decoded.as_str(), "d" | "D") {
            if let Some(goal_id) = selected {
                self.dispatch_async(GoalQueueManagerAction::Delete { goal_id });
            }
            return;
        }
        if let Some(goal_id) = self.moving_goal_id.clone() {
            let direction = if matches_editor_key(data, EditorKey::Up) {
                Some(GoalQueueMoveDirection::Up)
            } else if matches_editor_key(data, EditorKey::Down) {
                Some(GoalQueueMoveDirection::Down)
            } else {
                None
            };
            if let Some(direction) = direction {
                self.dispatch_async(GoalQueueManagerAction::Move { goal_id, direction });
                return;
            }
        }
        self.list.handle_key(data);
    }

    fn dispatch_async(&mut self, action: GoalQueueManagerAction) {
        self.pending_action = Some(action.clone());
        (self.on_action)(action);
    }

    fn render_manager(&self, width: usize) -> Vec<String> {
        let view = self.list.view();
        let hint = if self.moving_goal_id.is_some() {
            "↑↓ reorder · Space done · E edit · D delete · Esc cancel"
        } else {
            "↑↓ navigate · Space select · E edit · D delete · Esc cancel"
        };
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            current_theme().bold_fg(ColorToken::Primary, " Upcoming goals"),
            current_theme().fg(ColorToken::TextMuted, &format!(" {hint}")),
            String::new(),
        ];
        if self.goals.is_empty() {
            lines.push(current_theme().fg(ColorToken::TextMuted, "  No upcoming goals."));
        } else {
            for index in view.page.start..view.page.end {
                if let Some(goal) = view.items.get(index) {
                    lines.push(self.render_goal(goal, index, index == view.selected_index, width));
                }
            }
            let below = view.items.len().saturating_sub(view.page.end);
            if below > 0 {
                lines.push(String::new());
                lines.push(current_theme().fg(ColorToken::TextMuted, &format!(" ↓ {below} more")));
            }
        }
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));
        lines
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "…", false))
            .collect()
    }

    fn render_goal(
        &self,
        goal: &UpcomingGoal,
        index: usize,
        selected: bool,
        width: usize,
    ) -> String {
        let moving = self.moving_goal_id.as_deref() == Some(&goal.id);
        let pointer = if selected { SELECT_POINTER } else { " " };
        let prefix = current_theme().fg(
            if selected {
                ColorToken::Primary
            } else {
                ColorToken::TextDim
            },
            &format!("  {pointer} "),
        );
        let number = format!("{}. ", index + 1);
        let state = if moving { "  selected" } else { "" };
        let budget = width
            .saturating_sub(5 + visible_width(&number) + visible_width(state))
            .max(1);
        let objective = truncate_to_width(&format_objective(&goal.objective), budget, "…", false);
        let text = format!("{number}{objective}");
        let mut line = format!(
            "{prefix}{}",
            if selected {
                current_theme().bold_fg(ColorToken::Primary, &text)
            } else {
                current_theme().fg(ColorToken::Text, &text)
            }
        );
        if moving {
            line.push_str(&current_theme().fg(ColorToken::Success, state));
        }
        line
    }
}

impl Component for GoalQueueManagerComponent {
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

fn create_list(
    goals: &[UpcomingGoal],
    selected: Option<&str>,
    page_size: Option<isize>,
) -> SearchableList<UpcomingGoal> {
    let index = selected
        .and_then(|id| goals.iter().position(|goal| goal.id == id))
        .unwrap_or_default();
    SearchableList::new(
        goals.to_vec(),
        |goal: &UpcomingGoal| goal.objective.clone(),
        page_size,
        Some(isize::try_from(index).unwrap_or(isize::MAX)),
        false,
    )
}
fn format_objective(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    fn goal(id: &str, text: &str) -> UpcomingGoal {
        UpcomingGoal {
            id: id.to_owned(),
            objective: text.to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }
    #[test]
    fn dispatches_edit_delete_and_locks_until_completion() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let mut manager = GoalQueueManagerComponent::new(GoalQueueManagerOptions::new(
            vec![goal("a", "First")],
            move |a| called.lock().expect("events").push(a),
            || {},
        ));
        manager.handle_input_event("e");
        manager.handle_input_event("d");
        assert!(manager.is_busy());
        manager.handle_input_event("e");
        assert_eq!(events.lock().expect("events").len(), 2);
        manager.complete_action(Some(GoalQueueSnapshot { goals: Vec::new() }));
        assert!(!manager.is_busy());
        assert!(manager.goals.is_empty());
    }
    #[test]
    fn selects_move_mode_reorders_and_keeps_selection() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let called = Arc::clone(&events);
        let goals = vec![goal("a", "First"), goal("b", "Second")];
        let mut manager = GoalQueueManagerComponent::new(GoalQueueManagerOptions::new(
            goals.clone(),
            move |a| called.lock().expect("events").push(a),
            || {},
        ));
        manager.handle_input_event(" ");
        manager.handle_input_event("\u{1b}[B");
        assert!(manager.is_busy());
        assert!(
            matches!(&events.lock().expect("events")[0], GoalQueueManagerAction::Move { goal_id, direction: GoalQueueMoveDirection::Down } if goal_id == "a")
        );
        manager.complete_action(Some(GoalQueueSnapshot {
            goals: vec![goals[1].clone(), goals[0].clone()],
        }));
        assert_eq!(manager.selected_goal_id(), Some("a"));
    }
    #[test]
    fn renders_empty_paged_and_selected_rows_within_width() {
        let mut empty =
            GoalQueueManagerComponent::new(GoalQueueManagerOptions::new(Vec::new(), |_| {}, || {}));
        assert!(
            empty
                .render(30)
                .iter()
                .any(|l| strip(l).contains("No upcoming"))
        );
        let mut opts = GoalQueueManagerOptions::new(
            (0..4)
                .map(|i| goal(&format!("g{i}"), "long\n objective text"))
                .collect(),
            |_| {},
            || {},
        );
        opts.page_size = Some(2);
        let mut manager = GoalQueueManagerComponent::new(opts);
        manager.handle_input_event(" ");
        let lines = manager.render(34);
        assert!(lines.iter().any(|l| strip(l).contains("selected")));
        assert!(lines.iter().any(|l| strip(l).contains("2 more")));
        assert!(lines.iter().all(|l| visible_width(l) <= 34));
    }
    fn strip(text: &str) -> String {
        regex::Regex::new(r"\x1b\[[0-9;]*m")
            .expect("regex")
            .replace_all(text, "")
            .into_owned()
    }
}
