use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock};

use crate::agent_core_v2::kosong::contract::provider::{ChatProvider, ThinkingEffort};
use crate::agent_core_v2::kosong::protocol::identity::Protocol;
use crate::agent_core_v2::kosong::protocol::protocol_base::{
    ProtocolBaseContext, ProtocolBaseDefinition, ProtocolBaseRegistryError, register_protocol_base,
};
use crate::agent_core_v2::kosong::protocol::protocol_trait::trait_default_headers;
use crate::agent_core_v2::kosong::provider::bases::anthropic::anthropic::{
    AnthropicChatProvider, AnthropicOptions, get_anthropic_model_capability,
};
use crate::agent_core_v2::kosong::provider::bases::anthropic::anthropic_hooks::compose_anthropic_hooks;
use crate::agent_core_v2::kosong::provider::bases::openai::openai_hooks::{
    first_process_env, trait_endpoint, trait_provides,
};

fn string(values: &Map<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn strings(values: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    Some(
        values
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

fn string_map(values: &Map<String, Value>, key: &str) -> Option<IndexMap<String, String>> {
    Some(
        values
            .get(key)?
            .as_object()?
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
            .collect(),
    )
}

fn options_from_provides(values: Option<&Map<String, Value>>, model: &str) -> AnthropicOptions {
    let empty = Map::new();
    let values = values.unwrap_or(&empty);
    let mut options = AnthropicOptions::new(model);
    options.api_key = string(values, "apiKey");
    options.base_url = string(values, "baseUrl");
    options.stream = values.get("stream").and_then(Value::as_bool);
    options.default_max_tokens = values.get("defaultMaxTokens").and_then(Value::as_f64);
    options.beta_features = strings(values, "betaFeatures");
    options.default_headers = string_map(values, "defaultHeaders");
    options.metadata = string_map(values, "metadata");
    options.adaptive_thinking = values.get("adaptiveThinking").and_then(Value::as_bool);
    options.support_efforts = strings(values, "supportEfforts");
    options.beta_api = values.get("betaApi").and_then(Value::as_bool);
    options.thinking_effort = string(values, "thinkingEffort").map(ThinkingEffort::new);
    options
}

// Original: anthropic.contrib.ts, registerProtocolBase({ id: 'anthropic' })
pub fn anthropic_base_definition() -> ProtocolBaseDefinition {
    ProtocolBaseDefinition {
        id: Protocol::Anthropic,
        capability: Some(Arc::new(|model| {
            get_anthropic_model_capability(model).cloned()
        })),
        create_chat_provider: Arc::new(|context: &ProtocolBaseContext| {
            let endpoint = trait_endpoint(&context.traits);
            let provides = trait_provides(&context.traits);
            let mut options = options_from_provides(provides.as_ref(), &context.config.model_name);
            let api_key = context.config.api_key.clone().or_else(|| {
                endpoint
                    .as_ref()
                    .and_then(|endpoint| first_process_env(Some(endpoint.api_key_env.as_slice())))
            });
            if let Some(api_key) = api_key {
                options.api_key = Some(api_key);
            }
            let base_url = context.config.base_url.clone().or_else(|| {
                endpoint.as_ref().and_then(|endpoint| {
                    first_process_env(Some(endpoint.base_url_env.as_slice()))
                        .or_else(|| endpoint.default_base_url.clone())
                })
            });
            if let Some(base_url) = base_url {
                options.base_url = Some(base_url);
            }
            if let Some(headers) = trait_default_headers(&context.traits) {
                options.default_headers = Some(headers);
            }
            if let Some(config) = context.config.provider_options.as_ref() {
                if let Some(value) = config.default_max_tokens {
                    options.default_max_tokens = Some(value);
                }
                if let Some(value) = config.adaptive_thinking {
                    options.adaptive_thinking = Some(value);
                }
                if let Some(value) = config.support_efforts.as_ref() {
                    options.support_efforts = Some(value.clone());
                }
                if let Some(value) = config.beta_api {
                    options.beta_api = Some(value);
                }
                if let Some(value) = config.metadata.as_ref() {
                    options.metadata = Some(value.clone());
                }
            }
            options.hooks = compose_anthropic_hooks(&context.traits);
            Ok(Arc::new(AnthropicChatProvider::new(options)) as Arc<dyn ChatProvider>)
        }),
    }
}

static ANTHROPIC_BASE_REGISTERED: OnceLock<Result<(), ProtocolBaseRegistryError>> = OnceLock::new();

pub fn ensure_anthropic_base_registered() -> Result<(), ProtocolBaseRegistryError> {
    ANTHROPIC_BASE_REGISTERED
        .get_or_init(|| register_protocol_base(anthropic_base_definition()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::protocol::identity::{
        ProtocolAdapterConfig, ProtocolProviderOptions,
    };

    #[test]
    fn factory_applies_protocol_options_over_trait_provides() {
        let config = ProtocolAdapterConfig {
            protocol: Protocol::Anthropic,
            provider_type: None,
            base_url: Some("https://anthropic.example".to_owned()),
            model_name: "claude-opus-4-6".to_owned(),
            api_key: Some("key".to_owned()),
            default_headers: None,
            provider_options: Some(ProtocolProviderOptions {
                default_max_tokens: Some(8_192.0),
                adaptive_thinking: Some(true),
                support_efforts: Some(vec!["low".to_owned(), "max".to_owned()]),
                beta_api: Some(true),
                metadata: Some(IndexMap::from([("team".to_owned(), "core".to_owned())])),
                ..ProtocolProviderOptions::default()
            }),
        };
        let provider = (anthropic_base_definition().create_chat_provider)(&ProtocolBaseContext {
            config,
            traits: Vec::new(),
        })
        .unwrap();
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.model_name(), "claude-opus-4-6");
        assert_eq!(provider.max_completion_tokens(), Some(8_192.0));
    }
}
