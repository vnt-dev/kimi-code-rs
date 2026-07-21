use crate::{
    sdk::types::{ContextMessage, ContextMessageRole, PromptOriginKind},
    tui::{
        components::{Component, ComponentRole, Container},
        types::{SkillActivationTrigger, TranscriptEntry, TranscriptEntryKind},
    },
};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoAvailability {
    pub max_count: usize,
    pub stopped_at_compaction: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoChoice {
    pub id: String,
    pub count: usize,
    pub input: String,
    pub label: String,
}

/// Original: apps/kimi-code/src/tui/commands/undo.ts parseUndoCount()
pub fn parse_undo_count(args: &str) -> Option<usize> {
    let value = args.trim();
    if value.is_empty() {
        return Some(1);
    }
    if value.starts_with('0') || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let count = value.parse::<u64>().ok()?;
    (count <= MAX_SAFE_INTEGER)
        .then(|| usize::try_from(count).ok())
        .flatten()
}

/// Original: undo.ts undoAvailabilityFromContext()
pub fn undo_availability_from_context(history: &[ContextMessage]) -> UndoAvailability {
    let mut max_count = 0;
    let mut stopped_at_compaction = false;
    for message in history.iter().rev() {
        if message.origin.as_ref().map(|origin| origin.kind) == Some(PromptOriginKind::Injection) {
            continue;
        }
        if message.origin.as_ref().map(|origin| origin.kind)
            == Some(PromptOriginKind::CompactionSummary)
        {
            stopped_at_compaction = true;
            break;
        }
        if is_context_undo_anchor(message) {
            max_count += 1;
        }
    }
    UndoAvailability {
        max_count,
        stopped_at_compaction,
    }
}

pub fn is_context_undo_anchor(message: &ContextMessage) -> bool {
    if message.role != ContextMessageRole::User {
        return false;
    }
    let Some(origin) = &message.origin else {
        return true;
    };
    match origin.kind {
        PromptOriginKind::User => true,
        PromptOriginKind::SkillActivation | PromptOriginKind::PluginCommand => {
            origin
                .fields
                .get("trigger")
                .and_then(serde_json::Value::as_str)
                == Some("user-slash")
        }
        _ => false,
    }
}

/// Transcript-only counterpart of activeUndoAnchorEntries(). Component-backed
/// compaction markers will feed the same helpers once components are present.
pub fn active_undo_anchor_entries(entries: &[TranscriptEntry]) -> (Vec<&TranscriptEntry>, bool) {
    let last_compaction = entries
        .iter()
        .rposition(|entry| entry.compaction_data.is_some());
    let active = last_compaction.map_or(entries, |index| &entries[index + 1..]);
    (
        active
            .iter()
            .filter(|entry| is_undo_anchor_entry(entry))
            .collect(),
        last_compaction.is_some(),
    )
}

pub fn create_undo_choices(entries: &[TranscriptEntry], max_count: usize) -> Vec<UndoChoice> {
    if max_count == 0 {
        return Vec::new();
    }
    let (anchors, _) = active_undo_anchor_entries(entries);
    let start = anchors.len().saturating_sub(max_count);
    let anchors = &anchors[start..];
    anchors
        .iter()
        .enumerate()
        .map(|(index, entry)| UndoChoice {
            id: entry.id.clone(),
            count: anchors.len() - index,
            input: format_undo_choice_input(entry),
            label: format_undo_choice_label(entry),
        })
        .collect()
}

pub fn format_undo_choice_label(entry: &TranscriptEntry) -> String {
    match entry.kind {
        TranscriptEntryKind::SkillActivation => {
            let name = single_line(entry.skill_name.as_deref().unwrap_or_else(|| {
                entry
                    .content
                    .strip_prefix("Activated skill:")
                    .unwrap_or(&entry.content)
            }));
            let args = single_line(entry.skill_args.as_deref().unwrap_or_default());
            if name.is_empty() {
                "Skill: unknown".to_owned()
            } else if args.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {args}")
            }
        }
        TranscriptEntryKind::PluginCommand => entry
            .plugin_command_data
            .as_ref()
            .and_then(format_plugin_command_slash)
            .unwrap_or_else(|| "User message".to_owned()),
        _ => {
            let content = single_line(&entry.content);
            let image_count = entry.image_attachment_ids.as_ref().map_or(0, Vec::len);
            if !content.is_empty() {
                content
            } else if image_count > 0 {
                let noun = if image_count == 1 { "image" } else { "images" };
                format!("User message ({image_count} {noun})")
            } else {
                "User message".to_owned()
            }
        }
    }
}

pub fn format_undo_choice_input(entry: &TranscriptEntry) -> String {
    match entry.kind {
        TranscriptEntryKind::SkillActivation => {
            let name = single_line(entry.skill_name.as_deref().unwrap_or_else(|| {
                entry
                    .content
                    .strip_prefix("Activated skill:")
                    .unwrap_or(&entry.content)
            }));
            let args = single_line(entry.skill_args.as_deref().unwrap_or_default());
            if name.is_empty() {
                String::new()
            } else if args.is_empty() {
                format!("/{name}")
            } else {
                format!("/{name} {args}")
            }
        }
        TranscriptEntryKind::PluginCommand => entry
            .plugin_command_data
            .as_ref()
            .and_then(format_plugin_command_slash)
            .unwrap_or_else(|| entry.content.clone()),
        _ => entry.content.clone(),
    }
}

fn format_plugin_command_slash(
    data: &crate::tui::types::PluginCommandTranscriptData,
) -> Option<String> {
    let name = format!("{}:{}", data.plugin_id, data.command_name);
    if name.is_empty() {
        return None;
    }
    let args = single_line(data.args.as_deref().unwrap_or_default());
    Some(if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    })
}

fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn is_undo_anchor_entry(entry: &TranscriptEntry) -> bool {
    entry.kind == TranscriptEntryKind::User
        || (entry.kind == TranscriptEntryKind::SkillActivation
            && entry.skill_trigger == Some(SkillActivationTrigger::UserSlash))
        || entry.kind == TranscriptEntryKind::PluginCommand
}

pub fn find_undo_anchor_entry_index(entries: &[TranscriptEntry], count: usize) -> Option<usize> {
    let mut found = 0;
    for (index, entry) in entries.iter().enumerate().rev() {
        if is_undo_anchor_entry(entry) {
            found += 1;
            if found == count {
                return Some(index);
            }
        }
    }
    None
}

pub fn is_undo_context_entry(entry: &TranscriptEntry) -> bool {
    match entry.kind {
        TranscriptEntryKind::User
        | TranscriptEntryKind::Assistant
        | TranscriptEntryKind::ToolCall
        | TranscriptEntryKind::Thinking
        | TranscriptEntryKind::SkillActivation
        | TranscriptEntryKind::PluginCommand
        | TranscriptEntryKind::Cron => true,
        TranscriptEntryKind::Status | TranscriptEntryKind::Goal => entry.turn_id.is_some(),
        TranscriptEntryKind::Welcome => false,
    }
}

pub fn active_undo_anchor_components<'a>(
    entries: &'a [TranscriptEntry],
    children: &'a [Box<dyn Component>],
) -> (Vec<&'a TranscriptEntry>, bool) {
    if let Some(compaction_index) = children
        .iter()
        .rposition(|child| child.role() == ComponentRole::Compaction)
    {
        let anchors = children[compaction_index + 1..]
            .iter()
            .filter_map(|child| child.transcript_entry())
            .filter(|entry| is_undo_anchor_entry(entry))
            .collect();
        (anchors, true)
    } else {
        active_undo_anchor_entries(entries)
    }
}

pub fn find_undo_anchor_component_index(
    children: &[Box<dyn Component>],
    count: usize,
) -> Option<usize> {
    let mut found = 0;
    for (index, child) in children.iter().enumerate().rev() {
        if is_undo_anchor_component(child.as_ref()) {
            found += 1;
            if found == count {
                return Some(index);
            }
        }
    }
    None
}

pub fn remove_undo_context_components(container: &mut Container, start_index: usize) {
    if start_index >= container.children.len() {
        return;
    }
    for index in (start_index..container.children.len()).rev() {
        if is_undo_context_component(container.children[index].as_ref()) {
            container.children.remove(index);
        }
    }
}

pub fn is_undo_anchor_component(component: &dyn Component) -> bool {
    match component.role() {
        ComponentRole::UserMessage | ComponentRole::PluginCommand => true,
        ComponentRole::SkillActivation => component
            .transcript_entry()
            .is_some_and(|entry| entry.skill_trigger == Some(SkillActivationTrigger::UserSlash)),
        _ => false,
    }
}

pub fn is_undo_context_component(component: &dyn Component) -> bool {
    if let Some(entry) = component.transcript_entry() {
        return is_undo_context_entry(entry);
    }
    matches!(
        component.role(),
        ComponentRole::UserMessage
            | ComponentRole::AssistantMessage
            | ComponentRole::Thinking
            | ComponentRole::ToolCall
            | ComponentRole::AgentGroup
            | ComponentRole::AgentSwarmProgress
            | ComponentRole::ReadGroup
            | ComponentRole::SkillActivation
            | ComponentRole::PluginCommand
            | ComponentRole::BackgroundAgentStatus
            | ComponentRole::CronMessage
    )
}

pub fn format_undo_limit_message(requested_count: usize, availability: UndoAvailability) -> String {
    let reason = if availability.stopped_at_compaction {
        " after the last compaction"
    } else {
        ""
    };
    format!(
        "Cannot undo {}; only {} can be undone in the active context{reason}.",
        format_prompt_count(requested_count),
        format_prompt_count(availability.max_count)
    )
}

pub fn format_nothing_to_undo_message(availability: UndoAvailability) -> &'static str {
    if availability.stopped_at_compaction {
        "Nothing to undo after the last compaction."
    } else {
        "Nothing to undo."
    }
}

fn format_prompt_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "prompt" } else { "prompts" })
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use serde_json::{Map, json};

    use super::*;
    use crate::tui::utils::transcript_component_metadata::mark_transcript_component;
    use crate::{sdk::types::PromptOrigin, tui::types::TranscriptRenderMode};

    struct RoleComponent(ComponentRole);

    impl Component for RoleComponent {
        fn render(&mut self, _width: usize) -> Vec<String> {
            Vec::new()
        }
        fn invalidate(&mut self) {}
        fn role(&self) -> ComponentRole {
            self.0
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn entry(id: &str, kind: TranscriptEntryKind) -> TranscriptEntry {
        TranscriptEntry {
            id: id.to_owned(),
            kind,
            turn_id: None,
            render_mode: TranscriptRenderMode::Plain,
            content: String::new(),
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
        }
    }

    fn context(
        role: ContextMessageRole,
        kind: Option<PromptOriginKind>,
        trigger: Option<&str>,
    ) -> ContextMessage {
        let origin = kind.map(|kind| {
            let mut fields = Map::new();
            if let Some(trigger) = trigger {
                fields.insert("trigger".to_owned(), json!(trigger));
            }
            PromptOrigin { kind, fields }
        });
        ContextMessage {
            role,
            content: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            origin,
        }
    }

    #[test]
    fn parses_only_positive_javascript_safe_integers() {
        assert_eq!(parse_undo_count(""), Some(1));
        assert_eq!(parse_undo_count(" 2 "), Some(2));
        for invalid in ["0", "01", "-1", "1.5", "9007199254740992", "abc"] {
            assert_eq!(parse_undo_count(invalid), None);
        }
    }

    #[test]
    fn context_availability_skips_injections_and_stops_at_compaction() {
        let history = vec![
            context(ContextMessageRole::User, None, None),
            context(
                ContextMessageRole::User,
                Some(PromptOriginKind::CompactionSummary),
                None,
            ),
            context(
                ContextMessageRole::User,
                Some(PromptOriginKind::Injection),
                None,
            ),
            context(
                ContextMessageRole::User,
                Some(PromptOriginKind::SkillActivation),
                Some("model-tool"),
            ),
            context(
                ContextMessageRole::User,
                Some(PromptOriginKind::SkillActivation),
                Some("user-slash"),
            ),
            context(
                ContextMessageRole::User,
                Some(PromptOriginKind::PluginCommand),
                Some("user-slash"),
            ),
        ];
        assert_eq!(
            undo_availability_from_context(&history),
            UndoAvailability {
                max_count: 2,
                stopped_at_compaction: true
            }
        );
    }

    #[test]
    fn creates_reverse_counts_and_formats_anchor_labels() {
        let mut user = entry("user", TranscriptEntryKind::User);
        user.image_attachment_ids = Some(vec![1, 2]);
        let mut skill = entry("skill", TranscriptEntryKind::SkillActivation);
        skill.skill_name = Some("review".to_owned());
        skill.skill_args = Some(" src/app.rs  now ".to_owned());
        skill.skill_trigger = Some(SkillActivationTrigger::UserSlash);
        let mut plugin = entry("plugin", TranscriptEntryKind::PluginCommand);
        plugin.plugin_command_data = Some(crate::tui::types::PluginCommandTranscriptData {
            activation_id: "a".to_owned(),
            plugin_id: "demo".to_owned(),
            command_name: "deploy".to_owned(),
            args: Some(" prod ".to_owned()),
            trigger: SkillActivationTrigger::UserSlash,
        });
        let choices = create_undo_choices(&[user, skill, plugin], 3);
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.count)
                .collect::<Vec<_>>(),
            [3, 2, 1]
        );
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            [
                "User message (2 images)",
                "/review src/app.rs now",
                "/demo:deploy prod"
            ]
        );
    }

    #[test]
    fn limits_active_entries_after_last_compaction_and_formats_messages() {
        let before = entry("before", TranscriptEntryKind::User);
        let mut compaction = entry("compact", TranscriptEntryKind::Status);
        compaction.compaction_data = Some(Default::default());
        let after = entry("after", TranscriptEntryKind::User);
        let entries = [before, compaction, after];
        let (anchors, stopped) = active_undo_anchor_entries(&entries);
        assert!(stopped);
        assert_eq!(
            anchors
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["after"]
        );
        let availability = UndoAvailability {
            max_count: 1,
            stopped_at_compaction: true,
        };
        assert_eq!(
            format_undo_limit_message(2, availability),
            "Cannot undo 2 prompts; only 1 prompt can be undone in the active context after the last compaction."
        );
        assert_eq!(
            format_nothing_to_undo_message(availability),
            "Nothing to undo after the last compaction."
        );
    }

    #[test]
    fn component_anchors_stop_at_compaction_and_removal_preserves_notices() {
        let before = entry("before", TranscriptEntryKind::User);
        let after = entry("after", TranscriptEntryKind::User);
        let notice = entry("notice", TranscriptEntryKind::Status);
        let mut container = Container::new();
        container.add_child(mark_transcript_component(
            RoleComponent(ComponentRole::UserMessage),
            before.clone(),
        ));
        container.add_child(RoleComponent(ComponentRole::Compaction));
        container.add_child(mark_transcript_component(
            RoleComponent(ComponentRole::UserMessage),
            after.clone(),
        ));
        container.add_child(mark_transcript_component(
            RoleComponent(ComponentRole::Other),
            notice,
        ));
        container.add_child(RoleComponent(ComponentRole::Thinking));

        let entries = [before, after];
        let (anchors, stopped) = active_undo_anchor_components(&entries, &container.children);
        assert!(stopped);
        assert_eq!(
            anchors
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["after"]
        );
        let index = find_undo_anchor_component_index(&container.children, 1).expect("anchor");
        remove_undo_context_components(&mut container, index);
        assert_eq!(container.children.len(), 3);
        assert_eq!(
            container.children[2]
                .transcript_entry()
                .map(|entry| entry.id.as_str()),
            Some("notice")
        );
    }
}
