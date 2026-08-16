use std::sync::{Arc, LazyLock};

use crate::kosong::contract::{
    message::{ContentPart, Message, Role},
    tokens::{estimate_tokens, estimate_tokens_for_message, estimate_tokens_for_messages},
};

use super::types::{ContextMessage, PluginCommandTrigger, PromptOrigin, SkillActivationTrigger};

pub static COMPACTION_SUMMARY_PREFIX: LazyLock<&'static str> =
    LazyLock::new(|| include_str!("compaction-summary-prefix.md").trim_end());
pub const COMPACT_USER_MESSAGE_MAX_TOKENS: usize = 20_000;
pub const COMPACT_USER_MESSAGE_HEAD_TOKENS: usize = 2_000;
pub const COMPACTION_ELISION_VARIANT: &str = "compaction_elision";

#[derive(Clone, Debug, PartialEq)]
pub struct CompactionUserSelection {
    pub head: Vec<ContextMessage>,
    pub tail: Vec<ContextMessage>,
    pub elided: bool,
    pub omitted_tokens: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionShapeInput {
    pub summary: String,
    pub legacy_summary_message: Option<ContextMessage>,
    pub context_summary: Option<String>,
    pub compacted_count: u64,
    pub tokens_before: u64,
    pub tokens_after: Option<u64>,
    pub kept_user_message_count: Option<u64>,
    pub kept_head_user_message_count: Option<u64>,
    pub dropped_count: Option<u64>,
    pub legacy_tail: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionShape {
    pub summary: String,
    pub context_summary: String,
    pub compacted_count: u64,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub kept_user_message_count: u64,
    pub kept_head_user_message_count: Option<u64>,
    pub dropped_count: Option<u64>,
    pub messages: Vec<ContextMessage>,
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/compactionHandoff.ts
//   buildContextCompactionShape()
pub fn build_context_compaction_shape(
    history: &[ContextMessage],
    input: ContextCompactionShapeInput,
) -> ContextCompactionShape {
    if input.legacy_tail == Some(true) {
        let context_summary = input
            .context_summary
            .clone()
            .unwrap_or_else(|| input.summary.clone());
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(
            input
                .legacy_summary_message
                .unwrap_or_else(|| create_compaction_summary_message(&context_summary)),
        );
        messages.extend(
            history
                .iter()
                .skip(normalize_slice_start(input.compacted_count, history.len()))
                .cloned(),
        );
        let tokens_after = input.tokens_after.unwrap_or_else(|| {
            estimate_tokens_for_messages(messages.iter().map(|message| &message.message)) as u64
        });
        return ContextCompactionShape {
            summary: input.summary,
            context_summary,
            compacted_count: input.compacted_count,
            tokens_before: input.tokens_before,
            tokens_after,
            kept_user_message_count: 0,
            kept_head_user_message_count: None,
            dropped_count: input.dropped_count,
            messages,
        };
    }

    let compactable = collect_compactable_user_messages(history);
    let selection = select_compaction_user_messages(
        &compactable,
        COMPACT_USER_MESSAGE_MAX_TOKENS,
        COMPACT_USER_MESSAGE_HEAD_TOKENS,
    );
    let mut kept_messages = selection.head.clone();
    if selection.elided {
        kept_messages.push(create_compaction_elision_message(selection.omitted_tokens));
    }
    kept_messages.extend(selection.tail.clone());
    let context_summary = input
        .context_summary
        .unwrap_or_else(|| input.summary.clone());
    let tokens_after = input.tokens_after.unwrap_or_else(|| {
        (estimate_tokens(&context_summary)
            + estimate_tokens_for_messages(kept_messages.iter().map(|message| &message.message)))
            as u64
    });
    let kept_user_message_count = input
        .kept_user_message_count
        .unwrap_or((selection.head.len() + selection.tail.len()) as u64);
    let kept_head_user_message_count = input
        .kept_head_user_message_count
        .or_else(|| selection.elided.then_some(selection.head.len() as u64));
    kept_messages.push(create_compaction_summary_message(&context_summary));

    ContextCompactionShape {
        summary: input.summary,
        context_summary,
        compacted_count: input.compacted_count,
        tokens_before: input.tokens_before,
        tokens_after,
        kept_user_message_count,
        kept_head_user_message_count,
        dropped_count: input.dropped_count,
        messages: kept_messages,
    }
}

fn normalize_slice_start(index: u64, length: usize) -> usize {
    (index as usize).min(length)
}

// Original: compactionHandoff.ts, buildCompactionSummaryText().
pub fn build_compaction_summary_text(summary: &str) -> String {
    let suffix = summary.trim();
    format!(
        "{}\n{}",
        *COMPACTION_SUMMARY_PREFIX,
        if suffix.is_empty() {
            "(no summary available)"
        } else {
            suffix
        }
    )
}

pub fn create_compaction_summary_message(text: &str) -> ContextMessage {
    context_user_message(text, PromptOrigin::CompactionSummary)
}

pub fn create_compaction_elision_message(omitted_tokens: usize) -> ContextMessage {
    context_user_message(
        &build_compaction_elision_text(omitted_tokens),
        PromptOrigin::Injection {
            variant: COMPACTION_ELISION_VARIANT.to_owned(),
        },
    )
}

pub fn build_compaction_elision_text(omitted_tokens: usize) -> String {
    format!(
        "<system-reminder>\nSome of this conversation's user messages were omitted here during compaction: the messages above this note are the oldest user input, the messages below are the most recent, and roughly {omitted_tokens} tokens in between were dropped. The omitted content is covered by the compaction summary at the end of the conversation.\n</system-reminder>"
    )
}

fn context_user_message(text: &str, origin: PromptOrigin) -> ContextMessage {
    ContextMessage {
        message: Message::new(
            Role::User,
            vec![ContentPart::Text {
                text: text.to_owned(),
            }],
            Vec::new(),
        ),
        id: None,
        provider_message_id: None,
        origin: Some(origin),
        is_error: None,
        note: None,
        attachments: Vec::new(),
    }
}

pub fn collect_compactable_user_messages(messages: &[ContextMessage]) -> Vec<ContextMessage> {
    messages
        .iter()
        .filter(|message| is_real_user_input(message) && !is_compaction_summary_message(message))
        .cloned()
        .collect()
}

pub fn is_compaction_summary_message(message: &ContextMessage) -> bool {
    matches!(message.origin, Some(PromptOrigin::CompactionSummary))
}

pub fn is_real_user_input(message: &ContextMessage) -> bool {
    message.message.role == Role::User
        && compaction_user_message_disposition(message.origin.as_ref())
            == CompactionUserMessageDisposition::Keep
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompactionUserMessageDisposition {
    Keep,
    Drop,
}

pub fn compaction_user_message_disposition(
    origin: Option<&PromptOrigin>,
) -> CompactionUserMessageDisposition {
    match origin {
        None | Some(PromptOrigin::User) => CompactionUserMessageDisposition::Keep,
        Some(PromptOrigin::SkillActivation { trigger, .. }) => {
            if *trigger == SkillActivationTrigger::UserSlash {
                CompactionUserMessageDisposition::Keep
            } else {
                CompactionUserMessageDisposition::Drop
            }
        }
        Some(PromptOrigin::PluginCommand { trigger, .. }) => {
            if *trigger == PluginCommandTrigger::UserSlash {
                CompactionUserMessageDisposition::Keep
            } else {
                CompactionUserMessageDisposition::Drop
            }
        }
        Some(
            PromptOrigin::Injection { .. }
            | PromptOrigin::ShellCommand { .. }
            | PromptOrigin::CompactionSummary
            | PromptOrigin::SystemTrigger { .. }
            | PromptOrigin::Task { .. }
            | PromptOrigin::CronJob { .. }
            | PromptOrigin::CronMissed { .. }
            | PromptOrigin::HookResult { .. }
            | PromptOrigin::Retry { .. },
        ) => CompactionUserMessageDisposition::Drop,
    }
}

// Original: compactionHandoff.ts, selectRecentUserMessages().
pub fn select_recent_user_messages(
    messages: &[ContextMessage],
    max_tokens: usize,
) -> Vec<ContextMessage> {
    let mut selected = Vec::new();
    let mut remaining = max_tokens;
    for message in messages.iter().rev() {
        if remaining == 0 {
            break;
        }
        let tokens = estimate_tokens_for_message(&message.message);
        if tokens <= remaining {
            selected.push(message.clone());
            remaining -= tokens;
        } else {
            selected.push(truncate_user_message(message, remaining));
            break;
        }
    }
    selected.reverse();
    selected
}

// Original: compactionHandoff.ts, selectCompactionUserMessages().
pub fn select_compaction_user_messages(
    messages: &[ContextMessage],
    max_tokens: usize,
    head_tokens: usize,
) -> CompactionUserSelection {
    let total_tokens = messages
        .iter()
        .map(|message| estimate_tokens_for_message(&message.message))
        .sum::<usize>();
    if total_tokens <= max_tokens {
        return CompactionUserSelection {
            head: Vec::new(),
            tail: messages.to_vec(),
            elided: false,
            omitted_tokens: 0,
        };
    }

    let head_budget = head_tokens.min(max_tokens);
    let tail_budget = max_tokens - head_budget;
    let mut tail = Vec::new();
    let mut tail_remaining = tail_budget;
    let mut head_end_exclusive = messages.len();
    let mut tail_boundary_dropped_prefix = None;
    for (index, message) in messages.iter().enumerate().rev() {
        if tail_remaining == 0 {
            break;
        }
        let tokens = estimate_tokens_for_message(&message.message);
        if tokens <= tail_remaining {
            tail.push(message.clone());
            tail_remaining -= tokens;
            head_end_exclusive = index;
            continue;
        }
        let full_text = extract_text(&message.message.content);
        let kept_suffix = truncate_text_to_tokens_from_end(&full_text, tail_remaining);
        tail.push(replace_message_text(message, kept_suffix));
        head_end_exclusive = index;
        let dropped_prefix = &full_text[..full_text.len() - kept_suffix.len()];
        if !dropped_prefix.is_empty() {
            tail_boundary_dropped_prefix = Some(replace_message_text(message, dropped_prefix));
        }
        break;
    }
    tail.reverse();

    let mut head_candidates = messages[..head_end_exclusive].to_vec();
    if let Some(dropped_prefix) = tail_boundary_dropped_prefix {
        head_candidates.push(dropped_prefix);
    }
    let mut head = Vec::new();
    let mut head_remaining = head_budget;
    for message in &head_candidates {
        if head_remaining == 0 {
            break;
        }
        let tokens = estimate_tokens_for_message(&message.message);
        if tokens <= head_remaining {
            head.push(message.clone());
            head_remaining -= tokens;
        } else {
            head.push(truncate_user_message(message, head_remaining));
            break;
        }
    }

    let kept_tokens = head
        .iter()
        .chain(&tail)
        .map(|message| estimate_tokens_for_message(&message.message))
        .sum::<usize>();
    CompactionUserSelection {
        head,
        tail,
        elided: true,
        omitted_tokens: total_tokens.saturating_sub(kept_tokens),
    }
}

fn extract_text(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn truncate_text_to_tokens(text: &str, max_tokens: usize) -> &str {
    if max_tokens == 0 {
        return "";
    }
    let mut ascii_count = 0usize;
    let mut non_ascii_count = 0usize;
    let mut end = 0usize;
    for (index, character) in text.char_indices() {
        if character.is_ascii() {
            ascii_count += 1;
        } else {
            non_ascii_count += 1;
        }
        if ascii_count.div_ceil(4) + non_ascii_count > max_tokens {
            break;
        }
        end = index + character.len_utf8();
    }
    &text[..end]
}

fn truncate_text_to_tokens_from_end(text: &str, max_tokens: usize) -> &str {
    if max_tokens == 0 {
        return "";
    }
    let mut ascii_count = 0usize;
    let mut non_ascii_count = 0usize;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        if character.is_ascii() {
            ascii_count += 1;
        } else {
            non_ascii_count += 1;
        }
        if ascii_count.div_ceil(4) + non_ascii_count > max_tokens {
            break;
        }
        start = index;
    }
    &text[start..]
}

fn replace_message_text(message: &ContextMessage, text: &str) -> ContextMessage {
    let mut replaced = message.clone();
    replaced.message.content = Arc::new(vec![ContentPart::Text {
        text: text.to_owned(),
    }]);
    Arc::make_mut(&mut replaced.message.tool_calls).clear();
    replaced.attachments.clear();
    replaced
}

fn truncate_user_message(message: &ContextMessage, max_tokens: usize) -> ContextMessage {
    let text = extract_text(&message.message.content);
    replace_message_text(message, truncate_text_to_tokens(&text, max_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str, origin: Option<PromptOrigin>) -> ContextMessage {
        let mut message = context_user_message(text, PromptOrigin::User);
        message.origin = origin;
        message
    }

    fn text(message: &ContextMessage) -> String {
        extract_text(&message.message.content)
    }

    #[test]
    fn builds_summary_and_elision_messages_with_exact_origins() {
        assert_eq!(
            build_compaction_summary_text(" \n"),
            format!("{}\n(no summary available)", *COMPACTION_SUMMARY_PREFIX)
        );
        assert!(matches!(
            create_compaction_summary_message("summary").origin,
            Some(PromptOrigin::CompactionSummary)
        ));
        assert!(matches!(
            create_compaction_elision_message(42).origin,
            Some(PromptOrigin::Injection { ref variant }) if variant == COMPACTION_ELISION_VARIANT
        ));
        assert!(build_compaction_elision_text(42).contains("roughly 42 tokens"));
    }

    #[test]
    fn classifies_only_direct_user_actions_as_compactable() {
        assert_eq!(
            compaction_user_message_disposition(None),
            CompactionUserMessageDisposition::Keep
        );
        assert_eq!(
            compaction_user_message_disposition(Some(&PromptOrigin::SkillActivation {
                activation_id: "a".into(),
                skill_name: "s".into(),
                skill_args: None,
                trigger: SkillActivationTrigger::ModelTool,
                skill_type: None,
                skill_path: None,
                skill_source: None,
                skills: Vec::new(),
            })),
            CompactionUserMessageDisposition::Drop
        );
        assert!(!is_real_user_input(&context_user_message(
            "reminder",
            PromptOrigin::Injection {
                variant: "x".into()
            }
        )));
    }

    #[test]
    fn selects_oldest_head_and_newest_tail_with_an_elided_middle() {
        let messages = vec![
            user(&"a".repeat(40), None),
            user(&"b".repeat(40), None),
            user(&"c".repeat(40), None),
        ];
        let selection = select_compaction_user_messages(&messages, 15, 5);
        assert!(selection.elided);
        assert_eq!(selection.head.len(), 1);
        assert_eq!(text(&selection.head[0]), "a".repeat(20));
        assert_eq!(selection.tail.len(), 1);
        assert_eq!(text(&selection.tail[0]), "c".repeat(40));
        assert!(selection.omitted_tokens > 0);
    }

    #[test]
    fn truncates_from_unicode_scalar_boundaries() {
        assert_eq!(truncate_text_to_tokens("a😀b", 1), "a");
        assert_eq!(truncate_text_to_tokens_from_end("a😀b", 1), "b");
        assert_eq!(truncate_text_to_tokens("😀😀", 1), "😀");
        assert_eq!(truncate_text_to_tokens_from_end("😀😀", 1), "😀");
    }

    #[test]
    fn normalizes_legacy_slice_start() {
        assert_eq!(normalize_slice_start(1, 3), 1);
        assert_eq!(normalize_slice_start(2, 3), 2);
        assert_eq!(normalize_slice_start(10, 3), 3);
        assert_eq!(normalize_slice_start(0, 3), 0);
    }

    #[test]
    fn builds_current_and_legacy_handoff_shapes() {
        let history = vec![user("first", None), user("second", None)];
        let current = build_context_compaction_shape(
            &history,
            ContextCompactionShapeInput {
                summary: "summary".into(),
                legacy_summary_message: None,
                context_summary: None,
                compacted_count: 2,
                tokens_before: 10,
                tokens_after: Some(4),
                kept_user_message_count: None,
                kept_head_user_message_count: None,
                dropped_count: Some(0),
                legacy_tail: None,
            },
        );
        assert_eq!(current.messages.len(), 3);
        assert!(is_compaction_summary_message(
            current.messages.last().unwrap()
        ));
        assert_eq!(current.kept_user_message_count, 2);

        let legacy = build_context_compaction_shape(
            &history,
            ContextCompactionShapeInput {
                summary: "legacy".into(),
                legacy_summary_message: None,
                context_summary: None,
                compacted_count: 1,
                tokens_before: 10,
                tokens_after: Some(3),
                kept_user_message_count: Some(99),
                kept_head_user_message_count: Some(99),
                dropped_count: None,
                legacy_tail: Some(true),
            },
        );
        assert_eq!(legacy.messages.len(), 2);
        assert_eq!(legacy.kept_user_message_count, 0);
        assert_eq!(text(&legacy.messages[1]), "second");
    }
}
