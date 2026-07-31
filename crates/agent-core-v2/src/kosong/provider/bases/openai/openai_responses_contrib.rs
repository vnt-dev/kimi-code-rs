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
    first_process_env, trait_endpoint, trait_provides,
};
use crate::kosong::provider::bases::openai::openai_responses::{
    OpenAiResponsesChatProvider, OpenAiResponsesOptions, get_openai_responses_model_capability,
};

fn string(values: &Map<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(values: &Map<String, Value>, key: &str) -> Option<u64> {
    values.get(key).and_then(Value::as_u64)
}

fn headers(values: &Map<String, Value>, key: &str) -> Option<IndexMap<String, String>> {
    values.get(key)?.as_object().map(|headers| {
        headers
            .iter()
            .filter_map(|(name, value)| {
                value.as_str().map(|value| (name.clone(), value.to_owned()))
            })
            .collect()
    })
}

fn options_from_provides(
    values: Option<&Map<String, Value>>,
    model: &str,
) -> OpenAiResponsesOptions {
    let empty = Map::new();
    let values = values.unwrap_or(&empty);
    let mut options = OpenAiResponsesOptions::new(model);
    options.api_key = string(values, "apiKey");
    options.base_url = string(values, "baseUrl");
    options.max_output_tokens = number(values, "maxOutputTokens");
    options.thinking_effort = string(values, "thinkingEffort").map(ThinkingEffort::new);
    options.default_headers = headers(values, "defaultHeaders");
    options.tool_message_conversion = match values.get("toolMessageConversion") {
        Some(Value::String(value)) if value == "extract_text" => {
            Some(ToolMessageConversion::ExtractText)
        }
        Some(Value::Null) => Some(ToolMessageConversion::Parts),
        _ => None,
    };
    options
}

// Original: openai-responses.contrib.ts,
// registerProtocolBase({ id: 'openai_responses', ... })
pub fn openai_responses_base_definition() -> ProtocolBaseDefinition {
    ProtocolBaseDefinition {
        id: Protocol::OpenAiResponses,
        capability: Some(Arc::new(|model| {
            get_openai_responses_model_capability(model).cloned()
        })),
        create_chat_provider: Arc::new(|context: &ProtocolBaseContext| {
            let endpoint = trait_endpoint(&context.traits);
            let provides = trait_provides(&context.traits);
            let mut options = options_from_provides(provides.as_ref(), &context.config.model_name);

            // Config and resolved endpoint fields compactly overwrite
            // `provides`. An endpoint without a key deliberately writes an
            // empty sentinel so the provider cannot fall back to OPENAI_API_KEY.
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
            if let Some(default_headers) = trait_default_headers(&context.traits) {
                options.default_headers = Some(default_headers);
            }
            if let Some(max_tokens) = context
                .config
                .provider_options
                .as_ref()
                .and_then(|options| options.default_max_tokens)
            {
                options.max_output_tokens = Some(max_tokens);
            }
            Ok(Arc::new(OpenAiResponsesChatProvider::new(options)) as Arc<dyn ChatProvider>)
        }),
    }
}

static OPENAI_RESPONSES_BASE_REGISTERED: OnceLock<Result<(), ProtocolBaseRegistryError>> =
    OnceLock::new();

pub fn ensure_openai_responses_base_registered() -> Result<(), ProtocolBaseRegistryError> {
    OPENAI_RESPONSES_BASE_REGISTERED
        .get_or_init(|| register_protocol_base(openai_responses_base_definition()))
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
    fn factory_applies_config_over_provides_and_endpoint() {
        let config = ProtocolAdapterConfig {
            protocol: Protocol::OpenAiResponses,
            provider_type: Some("vendor".to_owned()),
            base_url: None,
            model_name: "o3".to_owned(),
            api_key: Some("config-key".to_owned()),
            default_headers: None,
            provider_options: Some(ProtocolProviderOptions {
                default_max_tokens: Some(4096),
                ..ProtocolProviderOptions::default()
            }),
        };
        let trait_context = TraitContext {
            config: config.clone(),
            provider_id: config.provider_type.clone(),
        };
        let traits = vec![ResolvedTrait {
            protocol_trait: Arc::new(ProtocolTrait {
                provides: Some(Arc::new(|_| {
                    Some(
                        serde_json::json!({
                            "maxOutputTokens": 1,
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
            context: trait_context,
        }];
        let definition = openai_responses_base_definition();
        let provider =
            (definition.create_chat_provider)(&ProtocolBaseContext { config, traits }).unwrap();

        assert_eq!(definition.id, Protocol::OpenAiResponses);
        assert_eq!(provider.name(), "openai-responses");
        assert_eq!(provider.model_name(), "o3");
        assert_eq!(provider.max_completion_tokens(), Some(4096));
        assert_eq!(provider.thinking_effort().unwrap().as_str(), "high");
    }
}
