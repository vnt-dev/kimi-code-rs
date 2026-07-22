use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock};

use crate::kosong::contract::provider::{ChatProvider, ThinkingEffort};
use crate::kosong::protocol::identity::Protocol;
use crate::kosong::protocol::protocol_base::{
    ProtocolBaseContext, ProtocolBaseDefinition, ProtocolBaseRegistryError, register_protocol_base,
};
use crate::kosong::protocol::protocol_trait::trait_default_headers;
use crate::kosong::provider::bases::openai::openai_common::ToolMessageConversion;
use crate::kosong::provider::bases::openai::openai_hooks::{
    compose_openai_chat_hooks, first_process_env, trait_endpoint, trait_provides,
};
use crate::kosong::provider::bases::openai::openai_legacy::{
    OpenAiLegacyChatProvider, OpenAiLegacyOptions, get_openai_legacy_model_capability,
};

fn string_field(values: &Map<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number_field(values: &Map<String, Value>, key: &str) -> Option<f64> {
    values.get(key).and_then(Value::as_f64)
}

fn headers_field(values: &Map<String, Value>, key: &str) -> Option<IndexMap<String, String>> {
    values.get(key)?.as_object().map(|headers| {
        headers
            .iter()
            .filter_map(|(name, value)| {
                value.as_str().map(|value| (name.clone(), value.to_owned()))
            })
            .collect()
    })
}

fn options_from_provides(values: Option<&Map<String, Value>>, model: &str) -> OpenAiLegacyOptions {
    let empty = Map::new();
    let values = values.unwrap_or(&empty);
    let mut options = OpenAiLegacyOptions::new(model);
    options.stream = values.get("stream").and_then(Value::as_bool);
    options.api_key = string_field(values, "apiKey");
    options.base_url = string_field(values, "baseUrl");
    options.default_headers = headers_field(values, "defaultHeaders");
    options.reasoning_key = string_field(values, "reasoningKey");
    options.thinking_effort = string_field(values, "thinkingEffort").map(ThinkingEffort::new);
    options.max_tokens = number_field(values, "maxTokens");
    options.tool_message_conversion = match values.get("toolMessageConversion") {
        Some(Value::String(value)) if value == "extract_text" => {
            Some(ToolMessageConversion::ExtractText)
        }
        Some(Value::Null) => Some(ToolMessageConversion::Parts),
        _ => None,
    };
    options
}

// Original:
//   openai-legacy.contrib.ts, registerProtocolBase({ id: 'openai', ... })
//
// Rust adaptation:
//   Module-evaluation registration is exposed as an exactly-once ensure
//   function. The definition constructor remains separate so embedders and
//   tests can populate isolated registries without touching process state.
pub fn openai_legacy_base_definition() -> ProtocolBaseDefinition {
    ProtocolBaseDefinition {
        id: Protocol::OpenAi,
        capability: Some(Arc::new(|model| {
            get_openai_legacy_model_capability(model).cloned()
        })),
        create_chat_provider: Arc::new(|context: &ProtocolBaseContext| {
            let endpoint = trait_endpoint(&context.traits);
            let provides = trait_provides(&context.traits);
            let mut options = options_from_provides(provides.as_ref(), &context.config.model_name);

            // These compact fields overwrite `provides` only when defined.
            // An endpoint with no resolved key deliberately writes an empty
            // sentinel, suppressing OpenAiLegacyChatProvider's OPENAI_API_KEY
            // fallback for vendor transports.
            let resolved_api_key = context
                .config
                .api_key
                .clone()
                .or_else(|| {
                    endpoint.as_ref().and_then(|endpoint| {
                        first_process_env(Some(endpoint.api_key_env.as_slice()))
                    })
                })
                .or_else(|| endpoint.as_ref().map(|_| String::new()));
            if let Some(api_key) = resolved_api_key {
                options.api_key = Some(api_key);
            }
            let resolved_base_url = context.config.base_url.clone().or_else(|| {
                endpoint.as_ref().and_then(|endpoint| {
                    first_process_env(Some(endpoint.base_url_env.as_slice()))
                        .or_else(|| endpoint.default_base_url.clone())
                })
            });
            if let Some(base_url) = resolved_base_url {
                options.base_url = Some(base_url);
            }
            if let Some(headers) = trait_default_headers(&context.traits) {
                options.default_headers = Some(headers);
            }
            if let Some(provider_options) = context.config.provider_options.as_ref() {
                if let Some(max_tokens) = provider_options.default_max_tokens {
                    options.max_tokens = Some(max_tokens);
                }
                if let Some(reasoning_key) = provider_options.reasoning_key.as_ref() {
                    options.reasoning_key = Some(reasoning_key.clone());
                }
            }
            options.hooks = compose_openai_chat_hooks(&context.traits);
            Ok(Arc::new(OpenAiLegacyChatProvider::new(options)) as Arc<dyn ChatProvider>)
        }),
    }
}

static OPENAI_LEGACY_BASE_REGISTERED: OnceLock<Result<(), ProtocolBaseRegistryError>> =
    OnceLock::new();

pub fn ensure_openai_legacy_base_registered() -> Result<(), ProtocolBaseRegistryError> {
    OPENAI_LEGACY_BASE_REGISTERED
        .get_or_init(|| register_protocol_base(openai_legacy_base_definition()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::protocol::identity::{ProtocolAdapterConfig, ProtocolProviderOptions};
    use crate::kosong::protocol::protocol_trait::{
        ProtocolEndpoint, ProtocolTrait, ResolvedTrait, TraitContext,
    };

    #[test]
    fn factory_applies_config_over_provides_and_endpoint_defaults() {
        let config = ProtocolAdapterConfig {
            protocol: Protocol::OpenAi,
            provider_type: Some("vendor".to_owned()),
            base_url: None,
            model_name: "gpt-5-mini".to_owned(),
            api_key: Some("config-key".to_owned()),
            default_headers: None,
            provider_options: Some(ProtocolProviderOptions {
                default_max_tokens: Some(4096.0),
                reasoning_key: Some(" reasoning ".to_owned()),
                ..ProtocolProviderOptions::default()
            }),
        };
        let context = TraitContext {
            config: config.clone(),
            provider_id: config.provider_type.clone(),
        };
        let traits = vec![ResolvedTrait {
            protocol_trait: Arc::new(ProtocolTrait {
                provides: Some(Arc::new(|_| {
                    Some(
                        serde_json::json!({
                            "stream": false,
                            "maxTokens": 1,
                            "thinkingEffort": "high"
                        })
                        .as_object()
                        .unwrap()
                        .clone(),
                    )
                })),
                endpoint: Some(Arc::new(|_| {
                    Some(ProtocolEndpoint {
                        api_key_env: None,
                        base_url_env: None,
                        default_base_url: Some("https://vendor.example/v1".to_owned()),
                    })
                })),
                ..ProtocolTrait::default()
            }),
            context,
        }];
        let definition = openai_legacy_base_definition();
        let provider =
            (definition.create_chat_provider)(&ProtocolBaseContext { config, traits }).unwrap();
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model_name(), "gpt-5-mini");
        assert_eq!(provider.max_completion_tokens(), Some(4096.0));
        assert_eq!(provider.thinking_effort().unwrap().as_str(), "high");
    }
}
