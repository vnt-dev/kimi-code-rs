use std::{collections::HashSet, sync::LazyLock};

use crate::tui::types::TranscriptEntry;

pub const TRANSCRIPT_WINDOW_ENABLED: bool = true;
pub static TRANSCRIPT_MAX_TURNS: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_MAX_TURNS", 15));
pub static TRANSCRIPT_EXPAND_TURNS: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_EXPAND_TURNS", 3));
pub static TRANSCRIPT_HYSTERESIS: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_HYSTERESIS", 5));
pub static TRANSCRIPT_KEEP_RECENT_STEPS: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_KEEP_RECENT_STEPS", 30));
pub static TRANSCRIPT_KEEP_RECENT_ASSISTANT: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_KEEP_RECENT_ASSISTANT", 20));
pub static TRANSCRIPT_KEEP_RECENT_ASSISTANT_COMPLETED: LazyLock<usize> =
    LazyLock::new(|| read_env_int("KIMI_CODE_TUI_KEEP_RECENT_ASSISTANT_COMPLETED", 2));

/// Original:
///   apps/kimi-code/src/tui/utils/transcript-window.ts
///   readEnvInt()
pub fn read_env_int(name: &str, fallback: usize) -> usize {
    std::env::var(name)
        .ok()
        .as_deref()
        .map(|value| read_env_int_from(Some(value), fallback))
        .unwrap_or(fallback)
}

pub fn read_env_int_from(raw: Option<&str>, fallback: usize) -> usize {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return fallback;
    };
    let Some(value) = parse_javascript_number(raw.trim()) else {
        return fallback;
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > usize::MAX as f64 {
        return fallback;
    }
    value as usize
}

fn parse_javascript_number(value: &str) -> Option<f64> {
    let unsigned = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    let sign = if value.starts_with('-') { -1.0 } else { 1.0 };
    let radix_value = if let Some(hex) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok().map(|value| value as f64)
    } else if let Some(binary) = unsigned
        .strip_prefix("0b")
        .or_else(|| unsigned.strip_prefix("0B"))
    {
        u64::from_str_radix(binary, 2)
            .ok()
            .map(|value| value as f64)
    } else if let Some(octal) = unsigned
        .strip_prefix("0o")
        .or_else(|| unsigned.strip_prefix("0O"))
    {
        u64::from_str_radix(octal, 8).ok().map(|value| value as f64)
    } else {
        return value.parse::<f64>().ok();
    };
    radix_value.map(|value| value * sign)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptTurn<'a> {
    pub turn_id: Option<&'a str>,
    pub entries: Vec<&'a TranscriptEntry>,
}

/// Original:
///   apps/kimi-code/src/tui/utils/transcript-window.ts
///   groupTurns()
pub fn group_turns(entries: &[TranscriptEntry]) -> Vec<TranscriptTurn<'_>> {
    let mut turns: Vec<TranscriptTurn<'_>> = Vec::new();
    let mut pending_undefined = Vec::new();

    for entry in entries {
        let Some(turn_id) = entry.turn_id.as_deref() else {
            pending_undefined.push(entry);
            continue;
        };
        if turns
            .last()
            .is_some_and(|current| current.turn_id == Some(turn_id))
        {
            if let Some(current) = turns.last_mut() {
                current.entries.push(entry);
            }
        } else {
            pending_undefined.push(entry);
            turns.push(TranscriptTurn {
                turn_id: Some(turn_id),
                entries: std::mem::take(&mut pending_undefined),
            });
        }
    }

    if !pending_undefined.is_empty() {
        turns.push(TranscriptTurn {
            turn_id: None,
            entries: pending_undefined,
        });
    }
    turns
}

/// Returns stable transcript entry IDs rather than object references. IDs are
/// the Rust identity boundary used by the mounted component registry.
///
/// Original:
///   apps/kimi-code/src/tui/utils/transcript-window.ts
///   turnsToTrim()
pub fn turns_to_trim<'a>(
    turns: &'a [TranscriptTurn<'a>],
    max_turns: usize,
    hysteresis: usize,
) -> HashSet<&'a str> {
    let mut to_remove = HashSet::new();
    if turns.len() <= max_turns.saturating_add(hysteresis) {
        return to_remove;
    }

    let mut remaining = turns.len();
    for turn in turns.iter().take(turns.len().saturating_sub(1)) {
        if remaining <= max_turns {
            break;
        }
        to_remove.extend(turn.entries.iter().map(|entry| entry.id.as_str()));
        remaining -= 1;
    }
    to_remove
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::types::{TranscriptEntryKind, TranscriptRenderMode};

    fn entry(id: &str, turn_id: Option<&str>, kind: TranscriptEntryKind) -> TranscriptEntry {
        TranscriptEntry {
            id: id.to_owned(),
            kind,
            turn_id: turn_id.map(str::to_owned),
            render_mode: TranscriptRenderMode::Markdown,
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

    #[test]
    fn groups_consecutive_entries_and_attaches_leading_undefined_entries() {
        let entries = [
            entry("1", None, TranscriptEntryKind::User),
            entry("2", Some("a"), TranscriptEntryKind::ToolCall),
            entry("3", Some("a"), TranscriptEntryKind::Assistant),
            entry("4", Some("b"), TranscriptEntryKind::Assistant),
        ];
        let turns = group_turns(&entries);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, Some("a"));
        assert_eq!(turns[0].entries.len(), 3);
        assert_eq!(turns[1].turn_id, Some("b"));
    }

    #[test]
    fn trailing_undefined_entries_become_their_own_turn() {
        let entries = [
            entry("1", Some("a"), TranscriptEntryKind::Assistant),
            entry("2", None, TranscriptEntryKind::User),
            entry("3", None, TranscriptEntryKind::Status),
        ];
        let turns = group_turns(&entries);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[1].turn_id, None);
        assert_eq!(turns[1].entries.len(), 2);
    }

    #[test]
    fn trims_oldest_turns_only_after_hysteresis() {
        let entries = [
            entry("1", Some("a"), TranscriptEntryKind::Assistant),
            entry("2", Some("b"), TranscriptEntryKind::Assistant),
            entry("3", Some("c"), TranscriptEntryKind::Assistant),
            entry("4", Some("d"), TranscriptEntryKind::Assistant),
        ];
        let turns = group_turns(&entries);

        assert!(turns_to_trim(&turns, 3, 1).is_empty());
        assert_eq!(turns_to_trim(&turns, 2, 0), HashSet::from(["1", "2"]));
    }

    #[test]
    fn never_trims_the_most_recent_turn() {
        let entries = (0..200)
            .map(|index| {
                entry(
                    &index.to_string(),
                    Some("solo"),
                    TranscriptEntryKind::ToolCall,
                )
            })
            .collect::<Vec<_>>();
        let turns = group_turns(&entries);

        assert!(turns_to_trim(&turns, 0, 0).is_empty());
    }

    #[test]
    fn reads_non_negative_javascript_integer_values() {
        assert_eq!(read_env_int_from(None, 7), 7);
        assert_eq!(read_env_int_from(Some("  "), 7), 7);
        assert_eq!(read_env_int_from(Some("42"), 7), 42);
        assert_eq!(read_env_int_from(Some("0"), 7), 0);
        assert_eq!(read_env_int_from(Some("1e2"), 7), 100);
        assert_eq!(read_env_int_from(Some("0x10"), 7), 16);
        assert_eq!(read_env_int_from(Some("-1"), 7), 7);
        assert_eq!(read_env_int_from(Some("1.5"), 7), 7);
        assert_eq!(read_env_int_from(Some("abc"), 7), 7);
    }
}
