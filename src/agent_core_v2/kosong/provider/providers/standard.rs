use std::sync::{Arc, OnceLock};

use crate::agent_core_v2::kosong::protocol::identity::Protocol;
use crate::agent_core_v2::kosong::protocol::protocol_trait::{ProtocolEndpoint, ProtocolTrait};

use super::super::provider_definition::{
    ProviderDefinition, ProviderDefinitionRegistryError, register_provider_definition,
};

fn endpoint(api_key_env: &str, base_url_env: &str) -> ProtocolEndpoint {
    ProtocolEndpoint {
        api_key_env: Some(api_key_env.to_owned()),
        base_url_env: Some(base_url_env.to_owned()),
        default_base_url: None,
    }
}

// Original:
//   packages/agent-core-v2/src/kosong/provider/providers/standard.contrib.ts
//
// Rust adaptation:
//   The definitions are constructed explicitly for testability. The ensure
//   function below uses OnceLock to reproduce JavaScript module evaluation's
//   exactly-once registration side effect.
pub fn standard_provider_definitions() -> Vec<ProviderDefinition> {
    vec![
        ProviderDefinition {
            id: "anthropic".to_owned(),
            base_protocol: Protocol::Anthropic,
            traits: Vec::new(),
            endpoint: Some(endpoint("ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL")),
            host_headers: None,
            model_source: None,
            capability: None,
        },
        ProviderDefinition {
            id: "openai".to_owned(),
            base_protocol: Protocol::OpenAi,
            traits: Vec::new(),
            endpoint: Some(endpoint("OPENAI_API_KEY", "OPENAI_BASE_URL")),
            host_headers: None,
            model_source: None,
            capability: None,
        },
        ProviderDefinition {
            id: "openai_responses".to_owned(),
            base_protocol: Protocol::OpenAiResponses,
            traits: Vec::new(),
            endpoint: Some(endpoint("OPENAI_API_KEY", "OPENAI_BASE_URL")),
            host_headers: None,
            model_source: None,
            capability: None,
        },
        ProviderDefinition {
            id: "google-genai".to_owned(),
            base_protocol: Protocol::GoogleGenAi,
            traits: vec![
                Arc::new(ProtocolTrait {
                    endpoint: Some(Arc::new(|_| {
                        Some(endpoint("VERTEXAI_API_KEY", "GOOGLE_VERTEX_BASE_URL"))
                    })),
                    ..ProtocolTrait::default()
                }),
                Arc::new(ProtocolTrait {
                    endpoint: Some(Arc::new(|_| {
                        Some(endpoint("GOOGLE_API_KEY", "GOOGLE_GEMINI_BASE_URL"))
                    })),
                    ..ProtocolTrait::default()
                }),
            ],
            endpoint: None,
            host_headers: None,
            model_source: None,
            capability: None,
        },
    ]
}

static STANDARD_PROVIDERS_REGISTERED: OnceLock<Result<(), ProviderDefinitionRegistryError>> =
    OnceLock::new();

pub fn ensure_standard_provider_definitions_registered()
-> Result<(), ProviderDefinitionRegistryError> {
    STANDARD_PROVIDERS_REGISTERED
        .get_or_init(|| {
            for definition in standard_provider_definitions() {
                register_provider_definition(definition)?;
            }
            Ok(())
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::provider::provider_definition::{
        ProviderDefinitionRegistry, ResolvedProviderEndpoint,
    };
    use indexmap::IndexMap;

    fn registry() -> ProviderDefinitionRegistry {
        let mut registry = ProviderDefinitionRegistry::default();
        for definition in standard_provider_definitions() {
            registry.register(definition).unwrap();
        }
        registry
    }

    #[test]
    fn canonical_definitions_keep_protocols_and_env_declarations() {
        let registry = registry();
        for (id, protocol, api_key_env, base_url_env) in [
            (
                "anthropic",
                Protocol::Anthropic,
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_BASE_URL",
            ),
            (
                "openai",
                Protocol::OpenAi,
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
            ),
            (
                "openai_responses",
                Protocol::OpenAiResponses,
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
            ),
        ] {
            let definition = registry.get(id, Some(protocol)).unwrap();
            let endpoint = definition.endpoint.as_ref().unwrap();
            assert_eq!(endpoint.api_key_env.as_deref(), Some(api_key_env));
            assert_eq!(endpoint.base_url_env.as_deref(), Some(base_url_env));
            assert!(endpoint.default_base_url.is_none());
            assert!(definition.traits.is_empty());
        }
    }

    #[test]
    fn google_endpoint_preserves_vertex_then_gemini_precedence() {
        let registry = registry();
        assert_eq!(
            registry.resolve_endpoint(
                "google-genai",
                &IndexMap::from([
                    ("VERTEXAI_API_KEY".to_owned(), "vertex".to_owned()),
                    ("GOOGLE_API_KEY".to_owned(), "google".to_owned()),
                ])
            ),
            ResolvedProviderEndpoint {
                api_key: Some("vertex".to_owned()),
                base_url: None,
            }
        );
        assert_eq!(
            registry.resolve_endpoint(
                "google-genai",
                &IndexMap::from([("GOOGLE_API_KEY".to_owned(), "google".to_owned())])
            ),
            ResolvedProviderEndpoint {
                api_key: Some("google".to_owned()),
                base_url: None,
            }
        );
    }

    #[test]
    fn exactly_once_registration_matches_contrib_module_evaluation() {
        ensure_standard_provider_definitions_registered().unwrap();
        ensure_standard_provider_definitions_registered().unwrap();
    }
}
