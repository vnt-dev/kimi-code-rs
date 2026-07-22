use serde_json::{Map, Value};

use crate::agent_core_v2::kosong::contract::errors::ChatProviderError;
use crate::agent_core_v2::kosong::contract::provider::{FinishReason, ResponseFormat};
use crate::agent_core_v2::kosong::provider::bases::anthropic::anthropic_profile::{
    AnthropicModelFamily, AnthropicModelVersion, parse_anthropic_model_version,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_common::NormalizedFinishReason;

pub type AnthropicGenerationKwargs = Map<String, Value>;

pub const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
pub const CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";
pub const CLEAR_THINKING_EDIT: &str = "clear_thinking_20251015";
pub const FALLBACK_MAX_TOKENS: f64 = 128_000.0;

// Original: anthropic.ts, normalizeAnthropicStopReason()
pub fn normalize_anthropic_stop_reason(raw: Option<&str>) -> NormalizedFinishReason {
    let Some(raw) = raw else {
        return NormalizedFinishReason {
            finish_reason: None,
            raw_finish_reason: None,
        };
    };
    let finish_reason = match raw {
        "end_turn" | "stop_sequence" => FinishReason::Completed,
        "max_tokens" => FinishReason::Truncated,
        "tool_use" => FinishReason::ToolCalls,
        "pause_turn" => FinishReason::Paused,
        "refusal" => FinishReason::Filtered,
        _ => FinishReason::Other,
    };
    NormalizedFinishReason {
        finish_reason: Some(finish_reason),
        raw_finish_reason: Some(raw.to_owned()),
    }
}

// Original: anthropic.ts, applyResponseFormat()
pub fn apply_response_format(
    kwargs: &mut AnthropicGenerationKwargs,
    format: Option<&ResponseFormat>,
) -> Result<(), ChatProviderError> {
    let Some(format) = format else {
        return Ok(());
    };
    let ResponseFormat::JsonSchema { json_schema } = format else {
        return Err(ChatProviderError::ChatProvider {
            message: "Anthropic provider requires a JSON schema for structured response output."
                .to_owned(),
        });
    };
    let mut output_config = kwargs
        .get("output_config")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    output_config.insert(
        "format".to_owned(),
        serde_json::json!({
            "type": "json_schema",
            "schema": json_schema.schema,
        }),
    );
    kwargs.insert("output_config".to_owned(), Value::Object(output_config));
    Ok(())
}

fn family_name(family: AnthropicModelFamily) -> &'static str {
    match family {
        AnthropicModelFamily::Opus => "opus",
        AnthropicModelFamily::Sonnet => "sonnet",
        AnthropicModelFamily::Haiku => "haiku",
        AnthropicModelFamily::Fable => "fable",
        AnthropicModelFamily::Mythos => "mythos",
    }
}

fn ceiling_for_key(key: &str) -> Option<f64> {
    Some(match key {
        "fable-5" | "mythos-5" | "opus-4-8" | "opus-4-7" | "opus-4-6" | "sonnet-5"
        | "sonnet-4-6" => 128_000.0,
        "opus-4-5" | "sonnet-4-5" | "sonnet-4-0" | "sonnet-4" | "haiku-4-5" | "haiku-4" => 64_000.0,
        "opus-4-1" | "opus-4-0" | "opus-4" => 32_000.0,
        "opus-3-5" | "sonnet-3-5" | "sonnet-3-7" | "haiku-3-5" => 8_192.0,
        "opus-3" | "sonnet-3" | "haiku-3" => 4_096.0,
        _ => return None,
    })
}

// Original: anthropic.ts, lookupClaudeCeiling()
pub fn lookup_claude_ceiling(version: AnthropicModelVersion) -> Option<f64> {
    let family = family_name(version.family);
    if let Some(minor) = version.minor {
        for candidate in (0..=minor).rev() {
            if let Some(ceiling) =
                ceiling_for_key(&format!("{family}-{}-{candidate}", version.major))
            {
                return Some(ceiling);
            }
        }
    }
    ceiling_for_key(&format!("{family}-{}", version.major))
}

// Original: anthropic.ts, resolveDefaultMaxTokens()
pub fn resolve_default_max_tokens(model: &str, override_tokens: Option<f64>) -> f64 {
    let ceiling = parse_anthropic_model_version(model, true).and_then(lookup_claude_ceiling);
    let Some(ceiling) = ceiling else {
        return override_tokens.unwrap_or(FALLBACK_MAX_TOKENS);
    };
    match override_tokens {
        None => ceiling,
        Some(value) if value.is_nan() => value,
        Some(value) => value.min(ceiling),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::provider::JsonSchemaDefinition;

    #[test]
    fn request_policy_preserves_finish_schema_and_token_rules() {
        for (raw, expected) in [
            ("end_turn", FinishReason::Completed),
            ("max_tokens", FinishReason::Truncated),
            ("tool_use", FinishReason::ToolCalls),
            ("pause_turn", FinishReason::Paused),
            ("refusal", FinishReason::Filtered),
            ("future_reason", FinishReason::Other),
        ] {
            assert_eq!(
                normalize_anthropic_stop_reason(Some(raw)).finish_reason,
                Some(expected)
            );
        }

        let mut kwargs = Map::from_iter([(
            "output_config".to_owned(),
            serde_json::json!({"effort":"high"}),
        )]);
        apply_response_format(
            &mut kwargs,
            Some(&ResponseFormat::JsonSchema {
                json_schema: JsonSchemaDefinition {
                    name: "ignored-by-anthropic".to_owned(),
                    schema: serde_json::json!({"type":"object"})
                        .as_object()
                        .unwrap()
                        .clone(),
                    strict: Some(true),
                    description: Some("ignored".to_owned()),
                },
            }),
        )
        .unwrap();
        assert_eq!(kwargs["output_config"]["effort"], "high");
        assert_eq!(kwargs["output_config"]["format"]["type"], "json_schema");
        assert!(apply_response_format(&mut kwargs, Some(&ResponseFormat::JsonObject)).is_err());

        assert_eq!(
            resolve_default_max_tokens("claude-opus-4-8", None),
            128_000.0
        );
        assert_eq!(
            resolve_default_max_tokens("claude-opus-4-9", Some(200_000.0)),
            128_000.0
        );
        assert_eq!(
            resolve_default_max_tokens("vendor-model", Some(12_345.0)),
            12_345.0
        );
        assert!(resolve_default_max_tokens("claude-opus-4-8", Some(f64::NAN)).is_nan());
    }
}
