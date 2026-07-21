use std::any::Any;

use crate::tui::{
    components::{
        Component, ComponentRole,
        render::{truncate_to_width, visible_width},
    },
    theme::{ColorToken, current_theme},
    types::QueuedMessage,
};

const ELLIPSIS: &str = "…";
const SELECT_POINTER: &str = "❯";

pub struct QueuePaneOptions {
    pub messages: Vec<QueuedMessage>,
    pub is_compacting: bool,
    pub is_streaming: bool,
    pub can_steer_immediately: bool,
}

/// Renders messages waiting behind the active turn.
///
/// Original: `src/tui/components/panes/queue-pane.ts`,
/// `QueuePaneComponent`.
pub struct QueuePaneComponent {
    messages: Vec<QueuedMessage>,
    hint: Option<&'static str>,
}

impl QueuePaneComponent {
    pub fn new(options: QueuePaneOptions) -> Self {
        let hint = if options.messages.is_empty() {
            None
        } else {
            let has_steerable = options.messages.iter().any(|message| !message.is_bash());
            let can_steer = options.can_steer_immediately && has_steerable;
            Some(if options.is_compacting && !options.is_streaming {
                "  ↑ to edit · will send after compaction"
            } else if can_steer {
                "  ↑ to edit · ctrl-s to steer immediately"
            } else {
                "  ↑ to edit · will send after current task"
            })
        };

        Self {
            messages: options.messages,
            hint,
        }
    }
}

impl Component for QueuePaneComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines = vec![theme.fg(ColorToken::Border, &"─".repeat(width))];

        for item in &self.messages {
            let single_line = item.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let prefix = format!("  {SELECT_POINTER} ");
            if item.is_bash() {
                let prompt = "$ ";
                let available_width = width
                    .saturating_sub(visible_width(&prefix) + visible_width(prompt))
                    .max(1);
                let truncated = truncate_to_width(&single_line, available_width, ELLIPSIS, false);
                lines.push(format!(
                    "{}{}",
                    theme.fg(ColorToken::Accent, &prefix),
                    theme.fg(ColorToken::ShellMode, &format!("{prompt}{truncated}"))
                ));
            } else {
                let available_width = width.saturating_sub(visible_width(&prefix)).max(1);
                let truncated = truncate_to_width(&single_line, available_width, ELLIPSIS, false);
                lines.push(theme.fg(ColorToken::Accent, &format!("{prefix}{truncated}")));
            }
        }

        if let Some(hint) = self.hint {
            lines.push(theme.fg(
                ColorToken::TextDim,
                &truncate_to_width(hint, width, ELLIPSIS, false),
            ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::components::render::visible_width;

    fn pane(
        messages: Vec<QueuedMessage>,
        is_compacting: bool,
        is_streaming: bool,
        can_steer_immediately: bool,
    ) -> QueuePaneComponent {
        QueuePaneComponent::new(QueuePaneOptions {
            messages,
            is_compacting,
            is_streaming,
            can_steer_immediately,
        })
    }

    #[test]
    fn renders_messages_and_immediate_steer_hint() {
        let mut component = pane(
            vec![
                QueuedMessage::prompt("first message"),
                QueuedMessage::prompt("/skill:review src/app.ts"),
            ],
            false,
            true,
            true,
        );
        let output = component.render(120).join("\n");
        assert!(output.contains("❯ first message"));
        assert!(output.contains("❯ /skill:review src/app.ts"));
        assert!(output.contains("ctrl-s to steer immediately"));
    }

    #[test]
    fn selects_compaction_or_current_task_hint() {
        let mut compacting = pane(
            vec![QueuedMessage::prompt("after compact")],
            true,
            false,
            true,
        );
        assert!(
            compacting
                .render(120)
                .join("\n")
                .contains("will send after compaction")
        );

        let mut disabled = pane(
            vec![QueuedMessage::prompt("after init")],
            false,
            true,
            false,
        );
        let output = disabled.render(120).join("\n");
        assert!(output.contains("will send after current task"));
        assert!(!output.contains("ctrl-s to steer immediately"));
    }

    #[test]
    fn truncates_long_messages_and_collapses_whitespace() {
        let mut long = pane(
            vec![QueuedMessage::prompt("a".repeat(200))],
            false,
            true,
            true,
        );
        let lines = long.render(30);
        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains(ELLIPSIS));
        assert!(visible_width(&lines[1]) <= 30);

        let mut multiline = pane(
            vec![QueuedMessage::prompt("line one\nline two\t  line three")],
            false,
            true,
            true,
        );
        let output = multiline.render(120).join("\n");
        assert!(output.contains("line one line two line three"));
    }

    #[test]
    fn bash_items_use_shell_prompt_and_are_not_steerable() {
        let mut only_bash = pane(vec![QueuedMessage::bash("ls -la")], false, true, true);
        let output = only_bash.render(120).join("\n");
        assert!(output.contains("❯ "));
        assert!(output.contains("$ ls -la"));
        assert!(output.contains("will send after current task"));
        assert!(!output.contains("ctrl-s to steer immediately"));

        let mut mixed = pane(
            vec![
                QueuedMessage::bash("ls"),
                QueuedMessage::prompt("focus on tests"),
            ],
            false,
            true,
            true,
        );
        assert!(
            mixed
                .render(120)
                .join("\n")
                .contains("ctrl-s to steer immediately")
        );
    }

    #[test]
    fn empty_queue_renders_only_border_without_hint() {
        let mut component = pane(Vec::new(), false, true, true);
        let lines = component.render(8);
        assert_eq!(lines.len(), 1);
        assert_eq!(visible_width(&lines[0]), 8);
    }
}
