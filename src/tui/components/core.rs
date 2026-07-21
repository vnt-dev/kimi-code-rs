use std::any::Any;

use crate::tui::types::TranscriptEntry;

pub const CURSOR_MARKER: &str = "\u{1b}_pi:c\u{7}";

/// Stable semantic replacement for JavaScript `instanceof` checks performed
/// by transcript operations such as `/undo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComponentRole {
    #[default]
    Other,
    Welcome,
    Compaction,
    UserMessage,
    AssistantMessage,
    Thinking,
    ToolCall,
    AgentGroup,
    AgentSwarmProgress,
    ReadGroup,
    SkillActivation,
    PluginCommand,
    BackgroundAgentStatus,
    CronMessage,
}

/// Original:
///   packages/pi-tui/src/tui.ts
///   Component
pub trait Component: Any + Send {
    fn render(&mut self, width: usize) -> Vec<String>;

    fn handle_input(&mut self, _data: &str) {}

    fn wants_key_release(&self) -> bool {
        false
    }

    fn invalidate(&mut self);

    fn role(&self) -> ComponentRole {
        ComponentRole::Other
    }

    fn transcript_entry(&self) -> Option<&TranscriptEntry> {
        None
    }

    fn as_any(&self) -> &dyn Any;
}

/// Original:
///   packages/pi-tui/src/tui.ts
///   Container
#[derive(Default)]
pub struct Container {
    pub children: Vec<Box<dyn Component>>,
}

impl Container {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_child(&mut self, component: impl Component + 'static) {
        self.children.push(Box::new(component));
    }

    pub fn add_boxed_child(&mut self, component: Box<dyn Component>) {
        self.children.push(component);
    }

    pub fn remove_child_at(&mut self, index: usize) -> Option<Box<dyn Component>> {
        (index < self.children.len()).then(|| self.children.remove(index))
    }

    pub fn clear(&mut self) {
        self.children.clear();
    }
}

impl Component for Container {
    fn render(&mut self, width: usize) -> Vec<String> {
        let width = width.max(1);
        let mut lines = Vec::new();
        for child in &mut self.children {
            lines.extend(child.render(width));
        }
        lines
    }

    fn invalidate(&mut self) {
        for child in &mut self.children {
            child.invalidate();
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct RecordingComponent {
        name: &'static str,
        widths: Arc<Mutex<Vec<usize>>>,
        invalidations: Arc<Mutex<usize>>,
    }

    impl Component for RecordingComponent {
        fn render(&mut self, width: usize) -> Vec<String> {
            if let Ok(mut widths) = self.widths.lock() {
                widths.push(width);
            }
            vec![format!("{}:{width}", self.name)]
        }

        fn invalidate(&mut self) {
            if let Ok(mut count) = self.invalidations.lock() {
                *count += 1;
            }
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn renders_children_in_order_and_clamps_zero_width() {
        let widths = Arc::new(Mutex::new(Vec::new()));
        let invalidations = Arc::new(Mutex::new(0));
        let mut container = Container::new();
        for name in ["first", "second"] {
            container.add_child(RecordingComponent {
                name,
                widths: Arc::clone(&widths),
                invalidations: Arc::clone(&invalidations),
            });
        }
        assert_eq!(container.render(0), ["first:1", "second:1"]);
        assert_eq!(
            widths.lock().map(|value| value.clone()).unwrap_or_default(),
            [1, 1]
        );
    }

    #[test]
    fn invalidates_clears_and_removes_by_index() {
        let widths = Arc::new(Mutex::new(Vec::new()));
        let invalidations = Arc::new(Mutex::new(0));
        let mut container = Container::new();
        container.add_child(RecordingComponent {
            name: "child",
            widths,
            invalidations: Arc::clone(&invalidations),
        });
        container.invalidate();
        assert_eq!(
            invalidations.lock().map(|value| *value).unwrap_or_default(),
            1
        );
        assert!(container.remove_child_at(1).is_none());
        assert!(container.remove_child_at(0).is_some());
        container.clear();
        assert!(container.children.is_empty());
    }
}
