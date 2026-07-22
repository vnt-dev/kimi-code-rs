use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use crate::agent_core_v2::kosong::contract::message::Message;
use crate::agent_core_v2::kosong::contract::provider::ToolCallIdPolicy;

const EMPTY_TOOL_CALL_ID: &str = "tool_call";

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/tool-call-id.ts
//   sanitizeToolCallId()
//
// Rust adaptation:
//   The JavaScript regex has no `u` flag and therefore replaces each UTF-16
//   code unit independently. Iterating encode_utf16 preserves that observable
//   behavior for non-BMP input such as emoji.
pub fn sanitize_tool_call_id(id: &str, max_length: Option<usize>) -> String {
    let mut sanitized = String::with_capacity(id.len());
    for unit in id.encode_utf16() {
        if (u16::from(b'0')..=u16::from(b'9')).contains(&unit)
            || (u16::from(b'a')..=u16::from(b'z')).contains(&unit)
            || (u16::from(b'A')..=u16::from(b'Z')).contains(&unit)
            || unit == u16::from(b'_')
            || unit == u16::from(b'-')
        {
            sanitized.push(char::from_u32(u32::from(unit)).unwrap());
        } else {
            sanitized.push('_');
        }
    }
    match max_length {
        Some(max_length) => sanitized.chars().take(max_length).collect(),
        None => sanitized,
    }
}

// Original: tool-call-id.ts, sanitizeOpenAIResponsesCallId()
pub fn sanitize_openai_responses_call_id(id: &str, max_length: Option<usize>) -> String {
    sanitize_tool_call_id(id.split('|').next().unwrap_or(id), max_length)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallIdError {
    pub max_length: usize,
    pub suffix: String,
}

impl fmt::Display for ToolCallIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Tool call id maxLength {} is too small for suffix {}.",
            self.max_length, self.suffix
        )
    }
}

impl Error for ToolCallIdError {}

// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/tool-call-id.ts
//   normalizeToolCallIdsForProvider()
//
// Rust adaptation:
//   The Vec is consumed and returned. Unchanged messages retain their value
//   identity; changed messages are cloned before rewriting, corresponding to
//   the source's object spread and producing a fresh per-message token cache.
pub fn normalize_tool_call_ids_for_provider(
    mut messages: Vec<Message>,
    policy: &ToolCallIdPolicy,
) -> Result<Vec<Message>, ToolCallIdError> {
    let raw_ids = collect_tool_call_ids(&messages);
    if raw_ids.is_empty() {
        return Ok(messages);
    }
    let mapped_ids = build_tool_call_id_map(&raw_ids, policy)?;

    for message in &mut messages {
        let tool_calls_changed = message.tool_calls.iter().any(|tool_call| {
            mapped_ids
                .get(&tool_call.id)
                .is_some_and(|mapped| mapped != &tool_call.id)
        });
        let mapped_tool_call_id = message
            .tool_call_id
            .as_ref()
            .and_then(|id| mapped_ids.get(id))
            .cloned()
            .or_else(|| message.tool_call_id.clone());
        let tool_call_id_changed = mapped_tool_call_id != message.tool_call_id;
        if !tool_calls_changed && !tool_call_id_changed {
            continue;
        }

        let mut normalized = message.clone();
        for tool_call in &mut normalized.tool_calls {
            if let Some(mapped) = mapped_ids.get(&tool_call.id) {
                tool_call.id.clone_from(mapped);
            }
        }
        normalized.tool_call_id = mapped_tool_call_id;
        *message = normalized;
    }
    Ok(messages)
}

fn collect_tool_call_ids(messages: &[Message]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for message in messages {
        for tool_call in &message.tool_calls {
            if seen.insert(tool_call.id.clone()) {
                ids.push(tool_call.id.clone());
            }
        }
        if let Some(id) = message.tool_call_id.as_ref()
            && seen.insert(id.clone())
        {
            ids.push(id.clone());
        }
    }
    ids
}

fn build_tool_call_id_map(
    raw_ids: &[String],
    policy: &ToolCallIdPolicy,
) -> Result<HashMap<String, String>, ToolCallIdError> {
    let mut mapped_ids = HashMap::new();
    let mut used_ids = HashSet::new();

    // Exact nonempty ids reserve their names before rewritten ids, preserving
    // the source's two-pass collision priority.
    for raw_id in raw_ids {
        let normalized = policy.normalize(raw_id);
        if normalized == *raw_id && !normalized.is_empty() {
            mapped_ids.insert(raw_id.clone(), normalized.clone());
            used_ids.insert(normalized);
        }
    }

    for raw_id in raw_ids {
        if mapped_ids.contains_key(raw_id) {
            continue;
        }
        let normalized = policy.normalize(raw_id);
        let unique = make_unique_tool_call_id(&normalized, &used_ids, policy.max_length)?;
        mapped_ids.insert(raw_id.clone(), unique.clone());
        used_ids.insert(unique);
    }
    Ok(mapped_ids)
}

fn make_unique_tool_call_id(
    normalized: &str,
    used_ids: &HashSet<String>,
    max_length: Option<usize>,
) -> Result<String, ToolCallIdError> {
    let base = if normalized.is_empty() {
        EMPTY_TOOL_CALL_ID
    } else {
        normalized
    };
    let candidate = truncate_tool_call_id(base, max_length, "")?;
    if !used_ids.contains(&candidate) {
        return Ok(candidate);
    }

    for ordinal in 2usize.. {
        let suffix = format!("_{ordinal}");
        let candidate = truncate_tool_call_id(base, max_length, &suffix)?;
        if !used_ids.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("unbounded suffix search always returns or errors")
}

fn truncate_tool_call_id(
    base: &str,
    max_length: Option<usize>,
    suffix: &str,
) -> Result<String, ToolCallIdError> {
    let Some(max_length) = max_length else {
        return Ok(format!("{base}{suffix}"));
    };
    let suffix_length = suffix.encode_utf16().count();
    if max_length <= suffix_length {
        return Err(ToolCallIdError {
            max_length,
            suffix: suffix.to_owned(),
        });
    }
    let base_length = max_length - suffix_length;
    let units: Vec<u16> = base.encode_utf16().take(base_length).collect();
    Ok(format!("{}{suffix}", String::from_utf16_lossy(&units)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::message::{
        ToolCall, ToolCallType, ToolOutput, create_assistant_message, create_tool_message,
    };
    use std::sync::Arc;

    fn policy(max_length: Option<usize>) -> ToolCallIdPolicy {
        ToolCallIdPolicy::new(
            Arc::new(move |id| sanitize_tool_call_id(id, max_length)),
            max_length,
        )
    }

    fn call(id: &str) -> ToolCall {
        ToolCall {
            call_type: ToolCallType::Function,
            id: id.to_owned(),
            name: "tool".to_owned(),
            arguments: Some("{}".to_owned()),
            extras: None,
            stream_index: None,
        }
    }

    #[test]
    fn sanitizer_preserves_javascript_utf16_and_length_behavior() {
        assert_eq!(sanitize_tool_call_id("call.a/b", None), "call_a_b");
        assert_eq!(sanitize_tool_call_id("a😀b", None), "a__b");
        assert_eq!(sanitize_tool_call_id("abcdef", Some(4)), "abcd");
        assert_eq!(
            sanitize_openai_responses_call_id("call-1|item-9", None),
            "call-1"
        );
        assert_eq!(sanitize_openai_responses_call_id("|item-9", None), "");
    }

    #[test]
    fn normalization_rewrites_calls_and_results_consistently() {
        let messages = vec![
            create_assistant_message(
                Vec::new(),
                Some(vec![call("bad!"), call("bad?"), call("safe")]),
            ),
            create_tool_message("bad!", ToolOutput::Text("a".to_owned())),
            create_tool_message("bad?", ToolOutput::Text("b".to_owned())),
        ];
        let normalized = normalize_tool_call_ids_for_provider(messages, &policy(None)).unwrap();
        assert_eq!(
            normalized[0]
                .tool_calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            ["bad_", "bad__2", "safe"]
        );
        assert_eq!(normalized[1].tool_call_id.as_deref(), Some("bad_"));
        assert_eq!(normalized[2].tool_call_id.as_deref(), Some("bad__2"));
    }

    #[test]
    fn exact_ids_reserve_collision_names_before_rewrites() {
        let messages = vec![create_assistant_message(
            Vec::new(),
            Some(vec![call("call_"), call("call!")]),
        )];
        let normalized = normalize_tool_call_ids_for_provider(messages, &policy(None)).unwrap();
        assert_eq!(normalized[0].tool_calls[0].id, "call_");
        assert_eq!(normalized[0].tool_calls[1].id, "call__2");
    }

    #[test]
    fn empty_ids_use_fallback_and_max_length_controls_suffixes() {
        let messages = vec![create_assistant_message(
            Vec::new(),
            Some(vec![call(""), call("?"), call("abcdef"), call("abcdef!")]),
        )];
        let normalized = normalize_tool_call_ids_for_provider(messages, &policy(Some(6))).unwrap();
        let ids: Vec<_> = normalized[0]
            .tool_calls
            .iter()
            .map(|call| call.id.as_str())
            .collect();
        assert_eq!(ids, ["tool_c", "_", "abcdef", "abcd_2"]);
        assert!(ids.iter().all(|id| id.len() <= 6));
    }

    #[test]
    fn too_small_max_length_returns_the_original_failure_message() {
        let messages = vec![create_assistant_message(
            Vec::new(),
            Some(vec![call("a!"), call("a?")]),
        )];
        let error = normalize_tool_call_ids_for_provider(messages, &policy(Some(2))).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Tool call id maxLength 2 is too small for suffix _2."
        );
    }
}
