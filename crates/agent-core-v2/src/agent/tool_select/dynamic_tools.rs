//! Pure helpers for dynamic-tool protocol context.
//!
//! Original: `toolSelect/dynamicTools.ts`.

use std::{collections::BTreeSet, sync::LazyLock};

use regex::Regex;

use crate::agent::context_memory::{ContextMessage, PromptOrigin};

pub const DYNAMIC_TOOL_SCHEMA_VARIANT: &str = "dynamic_tool_schema";
pub const LOADABLE_TOOLS_TRIGGER: &str = "loadable-tools";

static TOOLS_ADDED_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tools_added>\n?(.*?)\n?</tools_added>").expect("static regex is valid")
});
static TOOLS_REMOVED_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<tools_removed>\n?(.*?)\n?</tools_removed>").expect("static regex is valid")
});

pub fn is_dynamic_tool_schema_message(message: &ContextMessage) -> bool {
    message
        .message
        .tools
        .as_ref()
        .is_some_and(|tools| !tools.is_empty())
}

pub fn is_loadable_tools_announcement(message: &ContextMessage) -> bool {
    matches!(message.origin, Some(PromptOrigin::SystemTrigger { ref name }) if name == LOADABLE_TOOLS_TRIGGER)
}

pub fn strip_dynamic_tool_context(history: &[ContextMessage]) -> Vec<ContextMessage> {
    if !history.iter().any(|message| {
        is_dynamic_tool_schema_message(message) || is_loadable_tools_announcement(message)
    }) {
        return history.to_vec();
    }
    history
        .iter()
        .filter_map(|message| {
            if is_loadable_tools_announcement(message) {
                return None;
            }
            if !is_dynamic_tool_schema_message(message) {
                return Some(message.clone());
            }
            let mut stripped = message.clone();
            stripped.message.tools = None;
            (!stripped.message.content.is_empty() || !stripped.message.tool_calls.is_empty())
                .then_some(stripped)
        })
        .collect()
}

pub fn collect_loaded_dynamic_tool_names(history: &[ContextMessage]) -> BTreeSet<String> {
    history
        .iter()
        .flat_map(|message| message.message.tools.as_deref().into_iter().flatten())
        .map(|tool| tool.name.clone())
        .collect()
}

pub fn fold_announced_tool_names(history: &[ContextMessage]) -> BTreeSet<String> {
    let mut announced = BTreeSet::new();
    for message in history
        .iter()
        .filter(|message| is_loadable_tools_announcement(message))
    {
        let text = message
            .message
            .content
            .iter()
            .filter_map(|part| match part {
                crate::kosong::contract::message::ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        for name in match_tool_name_blocks(&text, &TOOLS_REMOVED_BLOCK) {
            announced.remove(&name);
        }
        for name in match_tool_name_blocks(&text, &TOOLS_ADDED_BLOCK) {
            announced.insert(name);
        }
    }
    announced
}

pub fn render_loadable_tools_announcement(added: &[String], removed: &[String]) -> String {
    let mut sections = Vec::new();
    if !added.is_empty() {
        sections.push(format!(
            "<tools_added>\n{}\n</tools_added>",
            added.join("\n")
        ));
    }
    if !removed.is_empty() {
        sections.push(format!(
            "<tools_removed>\n{}\n</tools_removed>",
            removed.join("\n")
        ));
    }
    sections.push("Use the select_tools tool with exact names to load full tool definitions before calling them. Names listed as removed are no longer loadable — do not select them. Fold all announcements in this conversation in order to get the current list.".into());
    sections.join("\n\n")
}

fn match_tool_name_blocks(text: &str, pattern: &Regex) -> Vec<String> {
    pattern
        .captures_iter(text)
        .flat_map(|capture| {
            capture
                .get(1)
                .into_iter()
                .flat_map(|body| body.as_str().lines())
        })
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent::context_memory::ContextMessage,
        kosong::contract::message::{ContentPart, Message, Role},
    };

    fn announcement(text: &str) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::System,
                vec![ContentPart::Text { text: text.into() }],
                vec![],
            ),
            id: None,
            provider_message_id: None,
            origin: Some(PromptOrigin::SystemTrigger {
                name: LOADABLE_TOOLS_TRIGGER.into(),
            }),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn folds_announcements_in_history_order_and_renders_protocol_text() {
        let names = fold_announced_tool_names(&[
            announcement("<tools_added>\nA\nB\n</tools_added>"),
            announcement("<tools_removed>\nA\n</tools_removed>\n<tools_added>\nC\n</tools_added>"),
        ]);
        assert_eq!(names, BTreeSet::from(["B".into(), "C".into()]));
        assert_eq!(
            render_loadable_tools_announcement(&["C".into()], &["A".into()]),
            "<tools_added>\nC\n</tools_added>\n\n<tools_removed>\nA\n</tools_removed>\n\nUse the select_tools tool with exact names to load full tool definitions before calling them. Names listed as removed are no longer loadable — do not select them. Fold all announcements in this conversation in order to get the current list."
        );
    }
}
