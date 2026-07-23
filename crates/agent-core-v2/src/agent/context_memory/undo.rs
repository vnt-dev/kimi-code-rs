use crate::kosong::contract::message::Role;

use super::types::{ContextMessage, PluginCommandTrigger, PromptOrigin, SkillActivationTrigger};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UndoCut {
    pub cut_index: i64,
    pub removed_count: usize,
    pub stopped_at_compaction: bool,
}

// Original:
//   packages/agent-core-v2/src/agent/contextMemory/contextOps.ts
//   computeUndoCut()
pub fn compute_undo_cut(state: &[ContextMessage], count: f64) -> UndoCut {
    let mut remaining = count;
    let mut cut_index = -1;
    let mut removed_count = 0usize;
    let mut stopped_at_compaction = false;
    for (index, message) in state.iter().enumerate().rev() {
        if remaining <= 0.0 {
            break;
        }
        if matches!(message.origin, Some(PromptOrigin::Injection { .. })) {
            continue;
        }
        if matches!(message.origin, Some(PromptOrigin::CompactionSummary)) {
            stopped_at_compaction = true;
            break;
        }
        if is_real_user_prompt(message) {
            remaining -= 1.0;
            removed_count += 1;
            cut_index = i64::try_from(index).unwrap_or(i64::MAX);
        }
    }
    UndoCut {
        cut_index,
        removed_count,
        stopped_at_compaction,
    }
}

pub fn is_fully_undoable(cut: UndoCut, count: f64) -> bool {
    cut.cut_index >= 0 && cut.removed_count as f64 >= count
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UndoUnavailableReason {
    Empty,
    CompactionBoundary,
    Insufficient,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UndoPrecheck {
    Ok,
    Unavailable {
        reason: UndoUnavailableReason,
        requested: f64,
        undoable: usize,
    },
}

// Original: contextOps.ts, precheckUndo().
pub fn precheck_undo(history: &[ContextMessage], count: f64) -> UndoPrecheck {
    let cut = compute_undo_cut(history, count);
    if is_fully_undoable(cut, count) {
        return UndoPrecheck::Ok;
    }
    let reason = if cut.stopped_at_compaction {
        UndoUnavailableReason::CompactionBoundary
    } else if cut.removed_count == 0 {
        UndoUnavailableReason::Empty
    } else {
        UndoUnavailableReason::Insufficient
    };
    UndoPrecheck::Unavailable {
        reason,
        requested: count,
        undoable: cut.removed_count,
    }
}

// Original: contextOps.ts, formatUndoUnavailableMessage().
pub fn format_undo_unavailable_message(precheck: UndoPrecheck) -> Option<String> {
    let UndoPrecheck::Unavailable {
        reason,
        requested,
        undoable,
    } = precheck
    else {
        return None;
    };
    Some(match reason {
        UndoUnavailableReason::Empty => "Nothing to undo: no user message to undo".to_owned(),
        UndoUnavailableReason::CompactionBoundary => {
            "Nothing to undo: would cross a compaction boundary".to_owned()
        }
        UndoUnavailableReason::Insufficient => format!(
            "Nothing to undo: only {undoable} of {} requested turn(s) available",
            js_number_to_string(requested)
        ),
    })
}

fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "Infinity".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Infinity".to_owned()
    } else if value == 0.0 {
        "0".to_owned()
    } else if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn is_real_user_prompt(message: &ContextMessage) -> bool {
    if message.message.role != Role::User {
        return false;
    }
    match message.origin.as_ref() {
        None | Some(PromptOrigin::User) => true,
        Some(PromptOrigin::SkillActivation { trigger, .. }) => {
            *trigger == SkillActivationTrigger::UserSlash
        }
        Some(PromptOrigin::PluginCommand { trigger, .. }) => {
            *trigger == PluginCommandTrigger::UserSlash
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{ContentPart, Message};

    fn message(role: Role, origin: Option<PromptOrigin>) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                role,
                vec![ContentPart::Text { text: "x".into() }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
        }
    }

    #[test]
    fn cuts_at_requested_user_prompt_and_keeps_trailing_assistant_in_cut() {
        let history = [
            message(Role::User, Some(PromptOrigin::User)),
            message(Role::Assistant, None),
            message(Role::Assistant, None),
        ];
        assert_eq!(
            compute_undo_cut(&history, 1.0),
            UndoCut {
                cut_index: 0,
                removed_count: 1,
                stopped_at_compaction: false,
            }
        );
        assert_eq!(precheck_undo(&history, 1.0), UndoPrecheck::Ok);
    }

    #[test]
    fn skips_injections_and_stops_at_compaction_boundary() {
        let history = [
            message(Role::User, Some(PromptOrigin::User)),
            message(Role::User, Some(PromptOrigin::CompactionSummary)),
            message(
                Role::User,
                Some(PromptOrigin::Injection {
                    variant: "reminder".into(),
                }),
            ),
            message(Role::Assistant, None),
        ];
        assert_eq!(
            precheck_undo(&history, 1.0),
            UndoPrecheck::Unavailable {
                reason: UndoUnavailableReason::CompactionBoundary,
                requested: 1.0,
                undoable: 0,
            }
        );
    }

    #[test]
    fn reports_empty_and_insufficient_histories() {
        assert_eq!(
            precheck_undo(&[], 1.0),
            UndoPrecheck::Unavailable {
                reason: UndoUnavailableReason::Empty,
                requested: 1.0,
                undoable: 0,
            }
        );
        let history = [message(Role::User, None), message(Role::Assistant, None)];
        let precheck = precheck_undo(&history, 2.0);
        assert_eq!(
            precheck,
            UndoPrecheck::Unavailable {
                reason: UndoUnavailableReason::Insufficient,
                requested: 2.0,
                undoable: 1,
            }
        );
        assert_eq!(
            format_undo_unavailable_message(precheck).as_deref(),
            Some("Nothing to undo: only 1 of 2 requested turn(s) available")
        );
    }

    #[test]
    fn preserves_fractional_javascript_count_behavior() {
        let history = [
            message(Role::User, None),
            message(Role::Assistant, None),
            message(Role::User, None),
        ];
        let cut = compute_undo_cut(&history, 1.5);
        assert_eq!(cut.cut_index, 0);
        assert_eq!(cut.removed_count, 2);
        assert!(is_fully_undoable(cut, 1.5));
    }
}
