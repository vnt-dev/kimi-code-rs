use serde_json::{Map, Value};
use std::sync::{Arc, LazyLock};

use crate::agent_core_v2::kosong::contract::provider::{ResponseFormat, ToolCallIdPolicy};
use crate::agent_core_v2::kosong::provider::bases::tool_call_id::sanitize_tool_call_id;

pub const KNOWN_REASONING_KEYS: [&str; 3] = ["reasoning_content", "reasoning_details", "reasoning"];
pub const DEFAULT_OUTBOUND_REASONING_KEY: &str = KNOWN_REASONING_KEYS[0];
pub const CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING: f64 = 128.0 * 1024.0;

pub static OPENAI_CHAT_TOOL_CALL_ID_POLICY: LazyLock<ToolCallIdPolicy> = LazyLock::new(|| {
    ToolCallIdPolicy::new(Arc::new(|id| sanitize_tool_call_id(id, Some(64))), Some(64))
});

pub type OpenAiLegacyGenerationKwargs = Map<String, Value>;

// Original: openai-legacy.ts, extractReasoningContent()
pub fn extract_reasoning_content(source: &Value, explicit_key: Option<&str>) -> Option<String> {
    let record = source.as_object()?;
    match explicit_key {
        Some(key) => record.get(key).and_then(Value::as_str).map(str::to_owned),
        None => KNOWN_REASONING_KEYS
            .iter()
            .find_map(|key| record.get(*key).and_then(Value::as_str).map(str::to_owned)),
    }
}

// Original: openai-legacy.ts, usesMaxCompletionTokens()
pub fn uses_max_completion_tokens(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    let reasoning_model = bytes.len() >= 2
        && bytes[0] == b'o'
        && bytes[1].is_ascii_digit()
        && (bytes.len() == 2 || matches!(bytes[2], b'-' | b'.'));
    let gpt_5 = normalized
        .strip_prefix("gpt-5")
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(['-', '.']));
    reasoning_model || gpt_5
}

// Original: openai-legacy.ts, completionTokenKwargs()
pub fn completion_token_kwargs(
    model: &str,
    max_completion_tokens: f64,
) -> OpenAiLegacyGenerationKwargs {
    let key = if uses_max_completion_tokens(model) {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    Map::from_iter([(key.to_owned(), Value::from(max_completion_tokens))])
}

// Original: openai-legacy.ts, normalizeGenerationKwargs()
pub fn normalize_generation_kwargs(
    model: &str,
    source: &OpenAiLegacyGenerationKwargs,
) -> OpenAiLegacyGenerationKwargs {
    let mut kwargs = source.clone();
    if uses_max_completion_tokens(model) {
        if !kwargs.contains_key("max_completion_tokens")
            && let Some(max_tokens) = kwargs.get("max_tokens").cloned()
        {
            kwargs.insert("max_completion_tokens".to_owned(), max_tokens);
        }
        kwargs.remove("max_tokens");
    }
    kwargs
}

// Original: openai-legacy.ts, responseFormatToOpenAI()
pub fn response_format_to_openai(format: &ResponseFormat) -> Map<String, Value> {
    match format {
        ResponseFormat::JsonObject => {
            Map::from_iter([("type".to_owned(), Value::String("json_object".to_owned()))])
        }
        ResponseFormat::JsonSchema { json_schema } => {
            let mut schema = Map::from_iter([
                ("name".to_owned(), Value::String(json_schema.name.clone())),
                (
                    "schema".to_owned(),
                    Value::Object(json_schema.schema.clone()),
                ),
            ]);
            if let Some(strict) = json_schema.strict {
                schema.insert("strict".to_owned(), Value::Bool(strict));
            }
            if let Some(description) = json_schema.description.as_ref() {
                schema.insert("description".to_owned(), Value::String(description.clone()));
            }
            Map::from_iter([
                ("type".to_owned(), Value::String("json_schema".to_owned())),
                ("json_schema".to_owned(), Value::Object(schema)),
            ])
        }
    }
}

// MIGRATION-TODO:
// Original: openai-legacy.ts, convertMessage(), convertHistoryMessages(),
// OpenAILegacyStreamedMessage, OpenAILegacyChatProvider and request methods.
// Missing dependency: the selected async OpenAI HTTP transport and its stream
// event types. Temporary behavior: none; this module exposes only completed
// pure request-shaping methods. Completion condition: port message/history
// shaping next, then implement the provider and stream over reqwest or a
// maintained SDK while preserving request order, cancellation and errors.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::provider::JsonSchemaDefinition;
    use serde_json::json;

    #[test]
    fn reasoning_extraction_honors_explicit_key_or_known_key_order() {
        let source = json!({
            "reasoning_details": "details",
            "reasoning": "fallback",
            "custom": "explicit"
        });
        assert_eq!(
            extract_reasoning_content(&source, None).as_deref(),
            Some("details")
        );
        assert_eq!(
            extract_reasoning_content(&source, Some("custom")).as_deref(),
            Some("explicit")
        );
        assert_eq!(extract_reasoning_content(&source, Some("missing")), None);
        assert_eq!(extract_reasoning_content(&Value::Null, None), None);
    }

    #[test]
    fn max_completion_field_model_rules_match_anchored_source_patterns() {
        for model in ["o1", "O3-mini", "o4.1", "gpt-5", "GPT-5-mini"] {
            assert!(uses_max_completion_tokens(model), "{model}");
        }
        for model in ["o", "o1preview", "gpt-50", "gpt-5x", "gpt-4o"] {
            assert!(!uses_max_completion_tokens(model), "{model}");
        }
        assert_eq!(
            completion_token_kwargs("o1", 8192.0),
            json!({"max_completion_tokens":8192.0})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            completion_token_kwargs("gpt-4o", 4096.0),
            json!({"max_tokens":4096.0}).as_object().unwrap().clone()
        );
    }

    #[test]
    fn generation_normalization_moves_legacy_field_only_when_needed() {
        let source = json!({"max_tokens":100,"temperature":0.5})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            normalize_generation_kwargs("o1", &source),
            json!({"max_completion_tokens":100,"temperature":0.5})
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(normalize_generation_kwargs("gpt-4o", &source), source);

        let explicit = json!({"max_tokens":100,"max_completion_tokens":200})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(
            normalize_generation_kwargs("gpt-5", &explicit),
            json!({"max_completion_tokens":200})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn response_formats_match_openai_request_wire_shape() {
        assert_eq!(
            response_format_to_openai(&ResponseFormat::JsonObject),
            json!({"type":"json_object"}).as_object().unwrap().clone()
        );
        let format = ResponseFormat::JsonSchema {
            json_schema: JsonSchemaDefinition {
                name: "answer".to_owned(),
                schema: json!({"type":"object"}).as_object().unwrap().clone(),
                strict: Some(true),
                description: Some("Structured answer".to_owned()),
            },
        };
        assert_eq!(
            response_format_to_openai(&format),
            json!({
                "type":"json_schema",
                "json_schema":{
                    "name":"answer",
                    "schema":{"type":"object"},
                    "strict":true,
                    "description":"Structured answer"
                }
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }

    #[test]
    fn chat_tool_call_policy_sanitizes_and_truncates_to_64_utf16_units() {
        let normalized =
            OPENAI_CHAT_TOOL_CALL_ID_POLICY.normalize(&format!("call/{}", "a".repeat(80)));
        assert_eq!(normalized.len(), 64);
        assert!(normalized.starts_with("call_"));
        assert_eq!(OPENAI_CHAT_TOOL_CALL_ID_POLICY.max_length, Some(64));
        assert_eq!(CHAT_COMPLETIONS_MAX_OUTPUT_TOKENS_CEILING, 131_072.0);
    }
}
