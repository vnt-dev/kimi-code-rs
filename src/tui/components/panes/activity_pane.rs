use std::any::Any;

use crate::tui::components::{Component, ComponentRole, chrome::moon_loader::MoonLoader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityPaneMode {
    Hidden,
    Waiting,
    Thinking,
    Composing,
    Tool,
}

pub struct ActivityPaneOptions {
    pub mode: ActivityPaneMode,
    pub spinner: Option<MoonLoader>,
    pub tip: Option<String>,
}

impl ActivityPaneOptions {
    pub fn new(mode: ActivityPaneMode) -> Self {
        Self {
            mode,
            spinner: None,
            tip: None,
        }
    }
}

/// Displays the live activity loader underneath the transcript.
///
/// Original:
/// `src/tui/components/panes/activity-pane.ts`, `ActivityPaneComponent`.
pub struct ActivityPaneComponent {
    spinner: Option<MoonLoader>,
    show_spinner: bool,
}

impl ActivityPaneComponent {
    pub fn new(mut options: ActivityPaneOptions) -> Self {
        let show_spinner = matches!(
            options.mode,
            ActivityPaneMode::Waiting | ActivityPaneMode::Composing | ActivityPaneMode::Tool
        ) && options.spinner.is_some();

        if show_spinner && let (Some(spinner), Some(tip)) = (&mut options.spinner, options.tip) {
            spinner.set_tip(format!(" · Tip: {tip}"));
        }

        Self {
            spinner: options.spinner,
            show_spinner,
        }
    }

    pub fn dispose(&mut self) {
        if let Some(spinner) = &mut self.spinner {
            spinner.dispose();
        }
    }
}

impl Component for ActivityPaneComponent {
    fn render(&mut self, width: usize) -> Vec<String> {
        let Some(spinner) = &mut self.spinner else {
            return Vec::new();
        };

        // The original updates the retained spinner even when the current mode
        // does not add it to the pane's child list.
        spinner.set_available_width(width);
        if !self.show_spinner {
            return Vec::new();
        }

        let mut lines = vec![String::new()];
        lines.extend(spinner.render(width));
        lines
    }

    fn invalidate(&mut self) {
        if let Some(spinner) = &mut self.spinner {
            spinner.invalidate();
        }
    }

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tui::components::chrome::moon_loader::SpinnerStyle;

    fn spinner(label: &str) -> MoonLoader {
        MoonLoader::new(Arc::new(|| {}), SpinnerStyle::Moon, None, label)
    }

    fn options(mode: ActivityPaneMode, tip: Option<&str>) -> ActivityPaneOptions {
        ActivityPaneOptions {
            mode,
            spinner: Some(spinner("working")),
            tip: tip.map(str::to_owned),
        }
    }

    #[test]
    fn visible_modes_render_spinner_after_spacer_with_optional_tip() {
        for mode in [
            ActivityPaneMode::Waiting,
            ActivityPaneMode::Tool,
            ActivityPaneMode::Composing,
        ] {
            let mut pane =
                ActivityPaneComponent::new(options(mode, Some("ctrl+s: steer mid-turn")));
            let lines = pane.render(80);
            assert_eq!(lines.first().map(String::as_str), Some(""));
            assert!(lines[1].contains("working"));
            assert!(lines[1].contains("Tip: ctrl+s: steer mid-turn"));
            pane.dispose();

            let mut pane = ActivityPaneComponent::new(options(mode, None));
            let lines = pane.render(80);
            assert_eq!(lines.first().map(String::as_str), Some(""));
            assert!(lines[1].contains("working"));
            assert!(!lines[1].contains("Tip:"));
            pane.dispose();
        }
    }

    #[test]
    fn hidden_and_thinking_modes_render_nothing() {
        for mode in [ActivityPaneMode::Hidden, ActivityPaneMode::Thinking] {
            let mut pane = ActivityPaneComponent::new(options(mode, Some("unused")));
            assert!(pane.render(80).is_empty());
            pane.dispose();
        }

        let mut pane =
            ActivityPaneComponent::new(ActivityPaneOptions::new(ActivityPaneMode::Waiting));
        assert!(pane.render(80).is_empty());
    }

    #[test]
    fn narrow_width_hides_tip_but_keeps_spinner() {
        for mode in [
            ActivityPaneMode::Waiting,
            ActivityPaneMode::Tool,
            ActivityPaneMode::Composing,
        ] {
            let mut pane =
                ActivityPaneComponent::new(options(mode, Some("ctrl+s: steer mid-turn")));
            let lines = pane.render(12);
            assert_eq!(lines.first().map(String::as_str), Some(""));
            assert!(lines[1].contains("working"));
            assert!(!lines[1].contains("Tip:"));
            pane.dispose();
        }
    }
}
