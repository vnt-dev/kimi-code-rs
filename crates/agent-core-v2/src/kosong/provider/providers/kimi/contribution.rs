use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::kosong::contract::capability::UNKNOWN_CAPABILITY;
use crate::kosong::contract::message::{ContentPart, Message, Role};
use crate::kosong::contract::provider::ThinkingEffort;
use crate::kosong::contract::tool::Tool;
use crate::kosong::protocol::identity::Protocol;
use crate::kosong::protocol::protocol_trait::{
    JsonObject, ProtocolEndpoint, ProtocolTrait, TraitContext, UsageExtraction,
};
use crate::kosong::provider::bases::openai::openai_common::tool_to_openai;
use crate::kosong::provider::config::ModelSource;
use crate::kosong::provider::provider_definition::{
    HostHeaders, ProviderDefinition, ProviderDefinitionRegistryError, register_provider_definition,
};

use super::files::{KimiFiles, KimiFilesOptions};
use super::schema::{KimiSchemaError, normalize_kimi_tool_schema};

pub const KIMI_API_KEY_ENV: &str = "KIMI_API_KEY";
pub const KIMI_BASE_URL_ENV: &str = "KIMI_BASE_URL";
pub const KIMI_DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/v1";
pub const KIMI_REASONING_KEY: &str = "reasoning_content";

const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

pub fn convert_kimi_tool(tool: &Tool) -> Result<JsonObject, KimiSchemaError> {
    if tool.name.starts_with('$') {
        return Ok(object(json!({
            "type": "builtin_function",
            "function": {"name": tool.name}
        })));
    }
    let mut converted = serde_json::to_value(tool_to_openai(tool))
        .expect("OpenAI tool parameters are JSON")
        .as_object()
        .cloned()
        .expect("OpenAI tool serialization is an object");
    converted
        .get_mut("function")
        .and_then(Value::as_object_mut)
        .expect("OpenAI function serialization is an object")
        .insert(
            "parameters".to_owned(),
            Value::Object(normalize_kimi_tool_schema(&tool.parameters)?),
        );
    Ok(converted)
}

fn endpoint() -> ProtocolEndpoint {
    ProtocolEndpoint {
        api_key_env: Some(KIMI_API_KEY_ENV.to_owned()),
        base_url_env: Some(KIMI_BASE_URL_ENV.to_owned()),
        default_base_url: Some(KIMI_DEFAULT_BASE_URL.to_owned()),
    }
}

fn is_effectively_empty_content(parts: &[ContentPart]) -> bool {
    parts.iter().all(|part| match part {
        ContentPart::Text { text } => text.trim().is_empty(),
        _ => false,
    })
}

fn convert_message(message: &Message, mut converted: JsonObject) -> JsonObject {
    if message.role == Role::Assistant && !message.tool_calls.is_empty() {
        let non_think_parts = message
            .content
            .iter()
            .filter(|part| !matches!(part, ContentPart::Think { .. }))
            .cloned()
            .collect::<Vec<_>>();
        if is_effectively_empty_content(&non_think_parts) {
            converted.remove("content");
        }
    }

    if let Some(Value::Array(converted_tool_calls)) = converted.get_mut("tool_calls") {
        for (index, tool_call) in message.tool_calls.iter().enumerate() {
            let Some(extras) = tool_call.extras.as_ref() else {
                continue;
            };
            if let Some(Value::Object(converted_tool_call)) = converted_tool_calls.get_mut(index) {
                converted_tool_call.insert("extras".to_owned(), Value::Object(extras.clone()));
            }
        }
    }

    if let Some(tools) = message.tools.as_ref().filter(|tools| !tools.is_empty()) {
        converted.insert(
            "tools".to_owned(),
            Value::Array(
                tools
                    .iter()
                    .map(|tool| {
                        convert_kimi_tool(tool)
                            .map(Value::Object)
                            // A declared conversion hook emits null on failure in
                            // the existing Rust hook contract, causing the request
                            // to be rejected rather than falling back to OpenAI.
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            ),
        );
    }
    converted
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FilesKey {
    api_key: Option<String>,
    base_url: String,
    headers: Vec<(String, String)>,
}

fn resolve_files(
    cache: &Mutex<HashMap<FilesKey, Arc<KimiFiles>>>,
    context: &TraitContext,
) -> Arc<KimiFiles> {
    let api_key = context
        .config
        .api_key
        .clone()
        .or_else(|| first_process_env(KIMI_API_KEY_ENV));
    let base_url = context
        .config
        .base_url
        .clone()
        .or_else(|| first_process_env(KIMI_BASE_URL_ENV))
        .unwrap_or_else(|| KIMI_DEFAULT_BASE_URL.to_owned());
    let key = FilesKey {
        api_key: api_key.clone(),
        base_url: base_url.clone(),
        headers: context
            .config
            .default_headers
            .as_ref()
            .map(|headers| {
                headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut cache = cache.lock();
    Arc::clone(cache.entry(key).or_insert_with(|| {
        Arc::new(KimiFiles::new(KimiFilesOptions {
            api_key,
            base_url,
            default_headers: context.config.default_headers.clone(),
            client_factory: None,
        }))
    }))
}

fn first_process_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

// Original: kimi.contrib.ts, kimiOpenAITrait.
pub fn kimi_openai_trait() -> ProtocolTrait {
    let files = Arc::new(Mutex::new(HashMap::new()));
    ProtocolTrait {
        strict_thinking_validation: Some(true),
        endpoint: Some(Arc::new(|_| Some(endpoint()))),
        cache_key: Some(Arc::new(|key, _| {
            Some(Map::from_iter([(
                "prompt_cache_key".to_owned(),
                Value::String(key.to_owned()),
            )]))
        })),
        with_thinking: Some(Arc::new(|effort, options, generation_kwargs, _| {
            let mut thinking = if effort.is_off() {
                object(json!({"type":"disabled"}))
            } else if effort.as_str() == "on" {
                object(json!({"type":"enabled"}))
            } else {
                object(json!({"type":"enabled","effort":effort.as_str()}))
            };
            if let Some(keep) = options.keep.as_ref() {
                thinking.insert("keep".to_owned(), Value::String(keep.clone()));
            }
            let mut extra_body = generation_kwargs
                .get("extra_body")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            extra_body.insert("thinking".to_owned(), Value::Object(thinking));
            Some(Map::from_iter([(
                "extra_body".to_owned(),
                Value::Object(extra_body),
            )]))
        })),
        preserve_thinking: Some(Arc::new(|generation_kwargs, _| {
            let thinking = generation_kwargs
                .get("extra_body")
                .and_then(Value::as_object)
                .and_then(|extra_body| extra_body.get("thinking"))
                .and_then(Value::as_object)?;
            (thinking.get("keep").and_then(Value::as_str) == Some("all")
                && thinking.get("type").and_then(Value::as_str) != Some("disabled"))
            .then_some(true)
        })),
        reasoning_key: Some(Arc::new(|_| Some(KIMI_REASONING_KEY.to_owned()))),
        with_max_completion_tokens: Some(Arc::new(|tokens, _| {
            Some(Map::from_iter([(
                "max_completion_tokens".to_owned(),
                Value::from(tokens),
            )]))
        })),
        build_params: Some(Arc::new(|mut params, _| {
            let extra_body = params.remove("extra_body");
            let max_tokens = params.remove("max_tokens");
            let max_completion_tokens = params.remove("max_completion_tokens");
            if let Some(tokens) = max_completion_tokens.or(max_tokens) {
                params.insert("max_completion_tokens".to_owned(), tokens);
            }
            if let Some(Value::Object(extra_body)) = extra_body {
                params.extend(extra_body);
            }
            Some(params)
        })),
        convert_tool: Some(Arc::new(|tool, _| convert_kimi_tool(tool).ok())),
        convert_message: Some(Arc::new(|message, converted, _| {
            Some(convert_message(message, converted))
        })),
        extract_usage: Some(Arc::new(|chunk, _| {
            if let Some(usage) = chunk.get("usage").and_then(Value::as_object) {
                return UsageExtraction::Usage(usage.clone());
            }
            if let Some(usage) = chunk
                .get("choices")
                .and_then(Value::as_array)
                .and_then(|choices| choices.first())
                .and_then(Value::as_object)
                .and_then(|choice| choice.get("usage"))
                .and_then(Value::as_object)
            {
                return UsageExtraction::Usage(usage.clone());
            }
            UsageExtraction::Defer
        })),
        upload_video: Some({
            let files = Arc::clone(&files);
            Arc::new(move |input, options, context| {
                let client = resolve_files(&files, context);
                Box::pin(async move { client.upload_video(input, options).await })
            })
        }),
        ..ProtocolTrait::default()
    }
}

// Original: kimi.contrib.ts, kimiAnthropicTrait.
pub fn kimi_anthropic_trait() -> ProtocolTrait {
    ProtocolTrait {
        with_thinking: Some(Arc::new(
            |effort: &ThinkingEffort, _, generation_kwargs, _| {
                let beta_features = generation_kwargs
                    .get("betaFeatures")
                    .and_then(Value::as_array)
                    .map(|features| {
                        features
                            .iter()
                            .filter(|feature| feature.as_str() != Some(INTERLEAVED_THINKING_BETA))
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut result = Map::from_iter([
                    (
                        "thinking".to_owned(),
                        json!({"type": if effort.is_off() {"disabled"} else {"enabled"}}),
                    ),
                    ("output_config".to_owned(), Value::Null),
                    ("betaFeatures".to_owned(), Value::Array(beta_features)),
                ]);
                if !effort.is_off() && effort.as_str() != "on" {
                    result.insert(
                        "output_config".to_owned(),
                        json!({"effort":effort.as_str()}),
                    );
                }
                Some(result)
            },
        )),
        ..ProtocolTrait::default()
    }
}

pub fn kimi_provider_definitions() -> Vec<ProviderDefinition> {
    vec![
        ProviderDefinition {
            id: "kimi".to_owned(),
            base_protocol: Protocol::OpenAi,
            traits: vec![Arc::new(kimi_openai_trait())],
            endpoint: Some(endpoint()),
            host_headers: Some(HostHeaders::Full),
            model_source: Some(ModelSource::OAuthCatalog),
            capability: Some(UNKNOWN_CAPABILITY),
        },
        ProviderDefinition {
            id: "kimi".to_owned(),
            base_protocol: Protocol::Anthropic,
            traits: vec![Arc::new(kimi_anthropic_trait())],
            endpoint: Some(endpoint()),
            host_headers: Some(HostHeaders::Full),
            model_source: Some(ModelSource::OAuthCatalog),
            capability: Some(UNKNOWN_CAPABILITY),
        },
    ]
}

static KIMI_PROVIDERS_REGISTERED: OnceLock<Result<(), ProviderDefinitionRegistryError>> =
    OnceLock::new();

pub fn ensure_kimi_provider_definitions_registered() -> Result<(), ProviderDefinitionRegistryError>
{
    KIMI_PROVIDERS_REGISTERED
        .get_or_init(|| {
            for definition in kimi_provider_definitions() {
                register_provider_definition(definition)?;
            }
            Ok(())
        })
        .clone()
}

fn object(value: Value) -> JsonObject {
    value.as_object().expect("literal is an object").clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::{ToolCall, ToolCallType};
    use crate::kosong::protocol::identity::ProtocolAdapterConfig;
    use crate::kosong::protocol::protocol_trait::ThinkingHookOptions;

    fn context(protocol: Protocol) -> TraitContext {
        TraitContext {
            config: ProtocolAdapterConfig {
                protocol,
                provider_type: Some("kimi".to_owned()),
                base_url: None,
                model_name: "kimi-k2".to_owned(),
                api_key: None,
                default_headers: None,
                provider_options: None,
            },
            provider_id: Some("kimi".to_owned()),
        }
    }

    #[test]
    fn converts_builtin_and_normalizes_regular_tools() {
        let builtin = Tool {
            name: "$web_search".to_owned(),
            description: "search".to_owned(),
            parameters: Map::new(),
            deferred: None,
        };
        assert_eq!(
            convert_kimi_tool(&builtin).unwrap(),
            object(json!({"type":"builtin_function","function":{"name":"$web_search"}}))
        );
        let regular = Tool {
            name: "read_file".to_owned(),
            description: "read".to_owned(),
            parameters: object(json!({
                "$defs":{"path":{"type":"string"}},
                "properties":{"path":{"$ref":"#/$defs/path"}}
            })),
            deferred: None,
        };
        assert_eq!(
            convert_kimi_tool(&regular).unwrap()["function"]["parameters"],
            json!({"properties":{"path":{"type":"string"}}})
        );
    }

    #[test]
    fn openai_hooks_match_request_reasoning_message_and_usage_behavior() {
        let protocol_trait = kimi_openai_trait();
        let context = context(Protocol::OpenAi);
        assert_eq!(
            protocol_trait.endpoint.as_ref().unwrap()(&context).unwrap(),
            endpoint()
        );
        let thinking = protocol_trait.with_thinking.as_ref().unwrap()(
            &ThinkingEffort::from("high"),
            &ThinkingHookOptions {
                keep: Some("all".to_owned()),
            },
            &Map::new(),
            &context,
        )
        .unwrap();
        assert_eq!(
            thinking,
            object(
                json!({"extra_body":{"thinking":{"type":"enabled","effort":"high","keep":"all"}}})
            )
        );
        assert_eq!(
            protocol_trait.preserve_thinking.as_ref().unwrap()(&thinking, &context),
            Some(true)
        );

        let mut message = Message::new(
            Role::Assistant,
            vec![ContentPart::Text {
                text: "  ".to_owned(),
            }],
            vec![ToolCall {
                call_type: ToolCallType::Function,
                id: "call_1".to_owned(),
                name: "read".to_owned(),
                arguments: Some("{}".to_owned()),
                extras: Some(object(json!({"a":1}))),
                stream_index: None,
            }],
        );
        message.tools = Some(Arc::new(vec![Tool {
            name: "$web_search".to_owned(),
            description: String::new(),
            parameters: Map::new(),
            deferred: None,
        }]));
        let converted = protocol_trait.convert_message.as_ref().unwrap()(
            &message,
            object(json!({"content":"  ","tool_calls":[{}]})),
            &context,
        )
        .unwrap();
        assert!(!converted.contains_key("content"));
        assert_eq!(converted["tool_calls"][0]["extras"], json!({"a":1}));
        assert_eq!(converted["tools"][0]["type"], "builtin_function");

        assert_eq!(
            protocol_trait.extract_usage.as_ref().unwrap()(
                &object(json!({"choices":[{"usage":{"prompt_tokens":5}}]})),
                &context
            ),
            UsageExtraction::Usage(object(json!({"prompt_tokens":5})))
        );
    }

    #[test]
    fn build_params_expands_extra_body_last_and_has_no_token_ceiling() {
        let protocol_trait = kimi_openai_trait();
        let context = context(Protocol::OpenAi);
        assert_eq!(
            protocol_trait.with_max_completion_tokens.as_ref().unwrap()(200_000, &context),
            Some(object(json!({"max_completion_tokens":200000})))
        );
        assert_eq!(
            protocol_trait.build_params.as_ref().unwrap()(
                object(json!({
                    "max_tokens":1024,
                    "max_completion_tokens":2048,
                    "temperature":0.5,
                    "extra_body":{"temperature":0.9}
                })),
                &context
            ),
            Some(object(
                json!({"max_completion_tokens":2048,"temperature":0.9})
            ))
        );
    }

    #[test]
    fn anthropic_thinking_strips_interleaved_beta() {
        let protocol_trait = kimi_anthropic_trait();
        let result = protocol_trait.with_thinking.as_ref().unwrap()(
            &ThinkingEffort::from("high"),
            &ThinkingHookOptions::default(),
            &object(json!({
                "betaFeatures":[INTERLEAVED_THINKING_BETA,"other-beta"]
            })),
            &context(Protocol::Anthropic),
        )
        .unwrap();
        assert_eq!(
            result,
            object(json!({
                "thinking":{"type":"enabled"},
                "output_config":{"effort":"high"},
                "betaFeatures":["other-beta"]
            }))
        );
        let on = protocol_trait.with_thinking.as_ref().unwrap()(
            &ThinkingEffort::from("on"),
            &ThinkingHookOptions::default(),
            &object(json!({"output_config":{"effort":"seeded"}})),
            &context(Protocol::Anthropic),
        )
        .unwrap();
        assert_eq!(on["output_config"], Value::Null);
        assert_eq!(protocol_trait.strict_thinking_validation, None);
    }

    #[test]
    fn definitions_share_vendor_level_facts_on_both_transports() {
        let definitions = kimi_provider_definitions();
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].base_protocol, Protocol::OpenAi);
        assert_eq!(definitions[1].base_protocol, Protocol::Anthropic);
        for definition in definitions {
            assert_eq!(definition.id, "kimi");
            assert_eq!(definition.endpoint, Some(endpoint()));
            assert_eq!(definition.host_headers, Some(HostHeaders::Full));
            assert_eq!(definition.model_source, Some(ModelSource::OAuthCatalog));
            assert_eq!(definition.capability, Some(UNKNOWN_CAPABILITY));
            assert_eq!(definition.traits.len(), 1);
        }
    }
}
