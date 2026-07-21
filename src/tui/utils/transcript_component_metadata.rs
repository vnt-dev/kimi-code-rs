use std::any::Any;

use crate::tui::{
    components::{Component, ComponentRole},
    types::TranscriptEntry,
};

/// Rust ownership makes the original WeakMap unnecessary: metadata travels
/// with the component and is released at the same time.
pub struct TranscriptComponent<C> {
    pub component: C,
    pub entry: TranscriptEntry,
}

/// Original: transcript-component-metadata.ts markTranscriptComponent()
pub fn mark_transcript_component<C: Component>(
    component: C,
    entry: TranscriptEntry,
) -> TranscriptComponent<C> {
    TranscriptComponent { component, entry }
}

/// Original: transcript-component-metadata.ts getTranscriptComponentEntry()
pub fn get_transcript_component_entry(component: &dyn Component) -> Option<&TranscriptEntry> {
    component.transcript_entry()
}

impl<C: Component> Component for TranscriptComponent<C> {
    fn render(&mut self, width: usize) -> Vec<String> {
        self.component.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.component.handle_input(data);
    }

    fn wants_key_release(&self) -> bool {
        self.component.wants_key_release()
    }

    fn invalidate(&mut self) {
        self.component.invalidate();
    }

    fn role(&self) -> ComponentRole {
        self.component.role()
    }

    fn transcript_entry(&self) -> Option<&TranscriptEntry> {
        Some(&self.entry)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::types::{TranscriptEntryKind, TranscriptRenderMode};

    struct Message;

    impl Component for Message {
        fn render(&mut self, width: usize) -> Vec<String> {
            vec![width.to_string()]
        }
        fn invalidate(&mut self) {}
        fn role(&self) -> ComponentRole {
            ComponentRole::UserMessage
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn owns_metadata_and_delegates_component_behavior() {
        let entry = TranscriptEntry {
            id: "entry-1".to_owned(),
            kind: TranscriptEntryKind::User,
            turn_id: None,
            render_mode: TranscriptRenderMode::Plain,
            content: "hello".to_owned(),
            model_text: None,
            color: None,
            detail: None,
            bullet: None,
            compaction_data: None,
            cron_data: None,
            background_agent_status: None,
            image_attachment_ids: None,
            skill_activation_id: None,
            skill_name: None,
            skill_args: None,
            skill_trigger: None,
            plugin_command_data: None,
        };
        let mut component = mark_transcript_component(Message, entry);
        assert_eq!(component.render(42), ["42"]);
        assert_eq!(component.role(), ComponentRole::UserMessage);
        assert_eq!(
            get_transcript_component_entry(&component).map(|entry| entry.id.as_str()),
            Some("entry-1")
        );
    }
}
