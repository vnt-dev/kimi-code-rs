use std::any::Any;

use crate::tui::components::{Component, ComponentRole};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepSummaryComponent {
    thinking: u64,
    tool: u64,
    message: u64,
}

impl StepSummaryComponent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.thinking == 0 && self.tool == 0 && self.message == 0
    }

    /// Original: step-summary.ts StepSummaryComponent.addCounts()
    pub fn add_counts(&mut self, thinking: u64, tool: u64, message: u64) {
        self.thinking = self.thinking.saturating_add(thinking);
        self.tool = self.tool.saturating_add(tool);
        self.message = self.message.saturating_add(message);
    }
}

impl Component for StepSummaryComponent {
    fn render(&mut self, _width: usize) -> Vec<String> {
        let mut parts = Vec::new();
        if self.thinking > 0 {
            parts.push(format!("thinking {} times", self.thinking));
        }
        if self.tool > 0 {
            parts.push(format!("call {} tools", self.tool));
        }
        if self.message > 0 {
            parts.push(format!("{} messages", self.message));
        }
        if parts.is_empty() {
            Vec::new()
        } else {
            vec![format!("\u{1b}[2m… {}\u{1b}[22m", parts.join(", "))]
        }
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

    #[test]
    fn empty_summary_renders_nothing() {
        let mut summary = StepSummaryComponent::new();
        assert!(summary.is_empty());
        assert!(summary.render(80).is_empty());
    }

    #[test]
    fn accumulates_counts_and_renders_nonzero_parts_in_fixed_order() {
        let mut summary = StepSummaryComponent::new();
        summary.add_counts(1, 2, 0);
        summary.add_counts(4, 48, 12);
        assert!(!summary.is_empty());
        assert_eq!(
            summary.render(1),
            ["\u{1b}[2m… thinking 5 times, call 50 tools, 12 messages\u{1b}[22m"]
        );
    }

    #[test]
    fn saturates_counters_instead_of_wrapping() {
        let mut summary = StepSummaryComponent::new();
        summary.add_counts(u64::MAX, 0, 0);
        summary.add_counts(1, 0, 0);
        assert!(summary.render(80)[0].contains(&u64::MAX.to_string()));
    }
}
