use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::sync::{Arc, OnceLock};

use crate::agent_core_v2::kosong::contract::provider::{ChatProvider, ThinkingEffort};
use crate::agent_core_v2::kosong::protocol::identity::Protocol;
use crate::agent_core_v2::kosong::protocol::protocol_base::{
    ProtocolBaseContext, ProtocolBaseDefinition, ProtocolBaseRegistryError, register_protocol_base,
};
use crate::agent_core_v2::kosong::protocol::protocol_trait::trait_default_headers;
use crate::agent_core_v2::kosong::provider::bases::google_genai::google_genai::{
    GoogleGenAiChatProvider, GoogleGenAiOptions, get_google_gen_ai_model_capability,
};
use crate::agent_core_v2::kosong::provider::bases::openai::openai_hooks::{
    first_process_env, trait_endpoint, trait_provides,
};

fn string(values: &Map<String, Value>, key: &str) -> Option<String> {
    values.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn headers(values: &Map<String, Value>) -> Option<IndexMap<String, String>> {
    Some(
        values
            .get("defaultHeaders")?
            .as_object()?
            .iter()
            .filter_map(|(name, value)| {
                value.as_str().map(|value| (name.clone(), value.to_owned()))
            })
            .collect(),
    )
}

fn options_from_provides(values: Option<&Map<String, Value>>, model: &str) -> GoogleGenAiOptions {
    let empty = Map::new();
    let values = values.unwrap_or(&empty);
    let mut options = GoogleGenAiOptions::new(model);
    options.api_key = string(values, "apiKey");
    options.base_url = string(values, "baseUrl");
    options.vertex_ai = values.get("vertexai").and_then(Value::as_bool);
    options.project = string(values, "project");
    options.location = string(values, "location");
    options.stream = values.get("stream").and_then(Value::as_bool);
    options.thinking_effort = string(values, "thinkingEffort").map(ThinkingEffort::new);
    options.default_headers = headers(values);
    options
}

// Original: google-genai.contrib.ts, registerProtocolBase({ id: 'google-genai' })
pub fn google_gen_ai_base_definition() -> ProtocolBaseDefinition {
    ProtocolBaseDefinition {
        id: Protocol::GoogleGenAi,
        capability: Some(Arc::new(|model| {
            get_google_gen_ai_model_capability(model).cloned()
        })),
        create_chat_provider: Arc::new(|context: &ProtocolBaseContext| {
            let endpoint = trait_endpoint(&context.traits);
            let provides = trait_provides(&context.traits);
            let mut options = options_from_provides(provides.as_ref(), &context.config.model_name);
            let api_key = context
                .config
                .api_key
                .clone()
                .or_else(|| {
                    endpoint.as_ref().and_then(|endpoint| {
                        first_process_env(Some(endpoint.api_key_env.as_slice()))
                    })
                })
                .or_else(|| endpoint.as_ref().map(|_| String::new()));
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
                if let Some(vertex_ai) = config.vertexai {
                    options.vertex_ai = Some(vertex_ai);
                }
                if let Some(project) = config.project.as_ref() {
                    options.project = Some(project.clone());
                }
                if let Some(location) = config.location.as_ref() {
                    options.location = Some(location.clone());
                }
            }
            Ok(Arc::new(GoogleGenAiChatProvider::new(options)) as Arc<dyn ChatProvider>)
        }),
    }
}

static GOOGLE_GEN_AI_BASE_REGISTERED: OnceLock<Result<(), ProtocolBaseRegistryError>> =
    OnceLock::new();

pub fn ensure_google_gen_ai_base_registered() -> Result<(), ProtocolBaseRegistryError> {
    GOOGLE_GEN_AI_BASE_REGISTERED
        .get_or_init(|| register_protocol_base(google_gen_ai_base_definition()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::protocol::identity::{
        ProtocolAdapterConfig, ProtocolProviderOptions,
    };

    #[test]
    fn factory_forwards_vertex_configuration() {
        let config = ProtocolAdapterConfig {
            protocol: Protocol::GoogleGenAi,
            provider_type: None,
            base_url: Some("https://vertex.example".to_owned()),
            model_name: "gemini-2.5-pro".to_owned(),
            api_key: Some("key".to_owned()),
            default_headers: None,
            provider_options: Some(ProtocolProviderOptions {
                vertexai: Some(true),
                project: Some("project".to_owned()),
                location: Some("global".to_owned()),
                ..ProtocolProviderOptions::default()
            }),
        };
        let provider =
            (google_gen_ai_base_definition().create_chat_provider)(&ProtocolBaseContext {
                config,
                traits: Vec::new(),
            })
            .unwrap();
        assert_eq!(provider.name(), "google_genai");
        assert_eq!(provider.model_name(), "gemini-2.5-pro");
        assert!(provider.max_completion_tokens().is_none());
    }
}
