use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole, render::truncate_to_width},
    theme::{ColorToken, current_theme},
};

const STATUS_BULLET: &str = "● ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmModeMarkerState {
    Active,
    Inactive,
    Ended,
}

impl SwarmModeMarkerState {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "Swarm activated",
            Self::Inactive => "Swarm deactivated",
            Self::Ended => "Swarm ended",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwarmModeMarkerComponent {
    state: SwarmModeMarkerState,
}

impl SwarmModeMarkerComponent {
    pub fn new(state: SwarmModeMarkerState) -> Self {
        Self { state }
    }
}

impl Component for SwarmModeMarkerComponent {
    /// Original: swarm-markers.ts SwarmModeMarkerComponent.render()
    fn render(&mut self, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }

        let token = if self.state == SwarmModeMarkerState::Inactive {
            ColorToken::TextDim
        } else {
            ColorToken::Success
        };
        let theme = current_theme();
        let line = format!(
            "{}{}",
            theme.bold_fg(token, STATUS_BULLET),
            theme.bold_fg(token, self.state.label())
        );
        vec![String::new(), truncate_to_width(&line, width, "…", false)]
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

    #[test]
    fn renders_original_labels_and_inactive_tone() {
        for (state, label) in [
            (SwarmModeMarkerState::Active, "Swarm activated"),
            (SwarmModeMarkerState::Inactive, "Swarm deactivated"),
            (SwarmModeMarkerState::Ended, "Swarm ended"),
        ] {
            let mut component = SwarmModeMarkerComponent::new(state);
            let lines = component.render(80);
            assert_eq!(lines[0], "");
            assert!(lines[1].contains(label));
        }
    }

    #[test]
    fn keeps_lines_within_narrow_widths() {
        let mut component = SwarmModeMarkerComponent::new(SwarmModeMarkerState::Active);
        for width in [0, 1, 2, 10, 39] {
            assert!(
                component
                    .render(width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }
}
