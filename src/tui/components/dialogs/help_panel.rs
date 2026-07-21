use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, render::truncate_to_width},
    keys::{EditorKey, ListKey, matches_editor_key, matches_list_key},
    theme::{ColorToken, current_theme},
    utils::printable_key::printable_char,
};

type CloseCallback = dyn FnMut() + Send;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardShortcut {
    pub keys: String,
    pub description: String,
}

impl KeyboardShortcut {
    pub fn new(keys: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            keys: keys.into(),
            description: description.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpPanelCommand {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
}

impl HelpPanelCommand {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            aliases: Vec::new(),
            description: description.into(),
        }
    }
}

pub fn default_keyboard_shortcuts() -> Vec<KeyboardShortcut> {
    [
        ("Shift-Tab", "Toggle plan mode"),
        ("Ctrl-G", "Edit in external editor ($VISUAL / $EDITOR)"),
        (
            "Ctrl-O",
            "Toggle tool output / compaction summary expansion",
        ),
        ("Ctrl-T", "Expand / collapse the todo list (when truncated)"),
        ("Ctrl-S", "Steer — inject a follow-up during streaming"),
        ("Shift-Enter / Ctrl-J", "Insert newline"),
        ("Ctrl-C", "Interrupt stream / clear input"),
        ("Ctrl-D", "Exit (on empty input)"),
        ("Esc", "Close dialogs / interrupt streaming"),
        ("↑ / ↓", "Browse input history"),
        ("Enter", "Submit"),
    ]
    .into_iter()
    .map(|(keys, description)| KeyboardShortcut::new(keys, description))
    .collect()
}

pub struct HelpPanelOptions {
    pub commands: Vec<HelpPanelCommand>,
    pub shortcuts: Option<Vec<KeyboardShortcut>>,
    pub max_visible: Option<usize>,
    on_close: Box<CloseCallback>,
}

impl HelpPanelOptions {
    pub fn new<C>(commands: Vec<HelpPanelCommand>, on_close: C) -> Self
    where
        C: FnMut() + Send + 'static,
    {
        Self {
            commands,
            shortcuts: None,
            max_visible: None,
            on_close: Box::new(on_close),
        }
    }
}

/// Scrollable keyboard and slash-command reference.
///
/// Original: `help-panel.ts`, `HelpPanelComponent`.
pub struct HelpPanelComponent {
    pub focused: bool,
    options: HelpPanelOptions,
    scroll_top: usize,
}

impl HelpPanelComponent {
    pub fn new(options: HelpPanelOptions) -> Self {
        Self {
            focused: false,
            options,
            scroll_top: 0,
        }
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn handle_input_event(&mut self, data: &str) {
        let printable = printable_char(data);
        if matches_editor_key(data, EditorKey::Escape)
            || matches_editor_key(data, EditorKey::Enter)
            || matches!(printable.as_str(), "q" | "Q")
        {
            (self.options.on_close)();
            return;
        }
        if matches_list_key(data, ListKey::Up) {
            self.scroll_top = self.scroll_top.saturating_sub(1);
        } else if matches_list_key(data, ListKey::Down) {
            self.scroll_top = self.scroll_top.saturating_add(1);
        } else if matches_list_key(data, ListKey::PageUp) {
            self.scroll_top = self.scroll_top.saturating_sub(10);
        } else if matches_list_key(data, ListKey::PageDown) {
            self.scroll_top = self.scroll_top.saturating_add(10);
        }
    }

    fn render_panel(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let shortcuts = self
            .options
            .shortcuts
            .clone()
            .unwrap_or_else(default_keyboard_shortcuts);
        let keyboard_width = shortcuts
            .iter()
            .map(|shortcut| shortcut.keys.chars().count())
            .max()
            .unwrap_or_default()
            .max(8);
        let mut commands = self.options.commands.clone();
        commands.sort_by(compare_slash_commands_for_display);
        let labels = commands.iter().map(command_label).collect::<Vec<_>>();
        let command_width = labels
            .iter()
            .map(|label| label.chars().count())
            .max()
            .unwrap_or_default()
            .max(12);
        let mut lines = vec![
            current_theme().fg(ColorToken::Primary, &"─".repeat(width)),
            format!(
                "{}{}",
                current_theme().bold_fg(ColorToken::Primary, " help "),
                current_theme().fg(
                    ColorToken::TextMuted,
                    "· Esc / Enter / q to cancel · ↑↓ scroll"
                )
            ),
            String::new(),
            format!(
                "  {}",
                current_theme().fg(
                    ColorToken::TextDim,
                    "Sure, Kimi is ready to help! Just send a message to get started."
                )
            ),
            String::new(),
            format!("  {}", current_theme().bold("Keyboard shortcuts")),
        ];
        lines.extend(shortcuts.iter().map(|shortcut| {
            format!(
                "    {}  {}",
                current_theme().fg(
                    ColorToken::Warning,
                    &format!("{:<keyboard_width$}", shortcut.keys)
                ),
                current_theme().fg(ColorToken::TextDim, &shortcut.description)
            )
        }));
        lines.push(String::new());
        lines.push(format!("  {}", current_theme().bold("Slash commands")));
        lines.extend(commands.iter().zip(labels).map(|(command, label)| {
            format!(
                "    {}  {}",
                current_theme().fg(ColorToken::Primary, &format!("{label:<command_width$}")),
                current_theme().fg(ColorToken::TextDim, &command.description)
            )
        }));
        lines.push(String::new());
        lines.push(current_theme().fg(ColorToken::Primary, &"─".repeat(width)));

        let max_visible = self.options.max_visible.unwrap_or(24).max(5);
        let output = if lines.len().saturating_sub(2) > max_visible {
            let content = &lines[1..lines.len() - 1];
            self.scroll_top = self
                .scroll_top
                .min(content.len().saturating_sub(max_visible));
            let end = (self.scroll_top + max_visible).min(content.len());
            let mut output = Vec::with_capacity(max_visible + 3);
            output.push(lines[0].clone());
            output.extend(content[self.scroll_top..end].iter().cloned());
            output.push(current_theme().fg(
                ColorToken::TextMuted,
                &format!(
                    " showing {}-{} of {}",
                    self.scroll_top + 1,
                    end,
                    content.len()
                ),
            ));
            output.push(lines.last().cloned().unwrap_or_default());
            output
        } else {
            self.scroll_top = 0;
            lines
        };
        output
            .into_iter()
            .map(|line| truncate_to_width(&line, width, "", false))
            .collect()
    }
}

impl Component for HelpPanelComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.render_panel(width)
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

fn command_label(command: &HelpPanelCommand) -> String {
    if command.aliases.is_empty() {
        format!("/{}", command.name)
    } else {
        format!(
            "/{} ({})",
            command.name,
            command
                .aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn compare_slash_commands_for_display(
    left: &HelpPanelCommand,
    right: &HelpPanelCommand,
) -> std::cmp::Ordering {
    slash_command_display_group(&left.name)
        .cmp(&slash_command_display_group(&right.name))
        .then_with(|| left.name.cmp(&right.name))
}

fn slash_command_display_group(name: &str) -> u8 {
    u8::from(name.starts_with("skill:"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tui::components::render::visible_width;

    use super::*;

    fn commands() -> Vec<HelpPanelCommand> {
        vec![
            HelpPanelCommand {
                name: "skill:zeta".to_owned(),
                aliases: Vec::new(),
                description: "Skill command".to_owned(),
            },
            HelpPanelCommand {
                name: "model".to_owned(),
                aliases: vec!["m".to_owned()],
                description: "Choose a model".to_owned(),
            },
            HelpPanelCommand::new("help", "Show help"),
        ]
    }

    #[test]
    fn renders_shortcuts_aliases_and_skill_commands_last() {
        let mut options = HelpPanelOptions::new(commands(), || {});
        options.shortcuts = Some(vec![KeyboardShortcut::new("Ctrl-X", "Example")]);
        options.max_visible = Some(100);
        let mut panel = HelpPanelComponent::new(options);
        let lines = panel.render(64);
        let plain = lines.iter().map(|line| strip_sgr(line)).collect::<Vec<_>>();
        assert!(plain.iter().any(|line| line.contains("Ctrl-X")));
        let help = plain
            .iter()
            .position(|line| line.contains("/help"))
            .expect("help");
        let model = plain
            .iter()
            .position(|line| line.contains("/model (/m)"))
            .expect("model");
        let skill = plain
            .iter()
            .position(|line| line.contains("/skill:zeta"))
            .expect("skill");
        assert!(help < model && model < skill);
        assert!(lines.iter().all(|line| visible_width(line) <= 64));
    }

    #[test]
    fn scrolls_clamps_and_keeps_borders_visible() {
        let mut options = HelpPanelOptions::new(commands(), || {});
        options.max_visible = Some(5);
        let mut panel = HelpPanelComponent::new(options);
        panel.handle_input_event("\u{1b}[6~");
        let lines = panel.render(52);
        assert!(panel.scroll_top() > 0);
        assert_eq!(lines.len(), 8);
        assert!(strip_sgr(&lines[0]).starts_with('─'));
        assert!(strip_sgr(&lines[lines.len() - 2]).contains("showing"));
        assert!(strip_sgr(lines.last().expect("bottom")).starts_with('─'));
        for _ in 0..100 {
            panel.handle_input_event("\u{1b}[B");
        }
        panel.render(52);
        let clamped = panel.scroll_top();
        panel.handle_input_event("\u{1b}[A");
        panel.render(52);
        assert_eq!(panel.scroll_top(), clamped.saturating_sub(1));
    }

    #[test]
    fn closes_for_escape_enter_and_plain_or_kitty_q() {
        let closes = Arc::new(Mutex::new(0usize));
        let callback = Arc::clone(&closes);
        let mut panel = HelpPanelComponent::new(HelpPanelOptions::new(commands(), move || {
            *callback.lock().expect("closes") += 1;
        }));
        for key in ["\u{1b}", "\r", "q", "Q", "\u{1b}[113u"] {
            panel.handle_input_event(key);
        }
        assert_eq!(*closes.lock().expect("closes"), 5);
    }

    #[test]
    fn default_shortcut_catalog_keeps_original_order() {
        let shortcuts = default_keyboard_shortcuts();
        assert_eq!(
            shortcuts.first().map(|item| item.keys.as_str()),
            Some("Shift-Tab")
        );
        assert_eq!(
            shortcuts.last().map(|item| item.keys.as_str()),
            Some("Enter")
        );
        assert_eq!(shortcuts.len(), 11);
    }

    fn strip_sgr(text: &str) -> String {
        let regex = regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid SGR regex");
        regex.replace_all(text, "").into_owned()
    }
}
