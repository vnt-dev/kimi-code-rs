use indexmap::IndexMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};

use crate::agent_core_v2::kosong::contract::capability::ModelCapability;
use crate::agent_core_v2::kosong::protocol::identity::{Protocol, ProtocolAdapterConfig};
use crate::agent_core_v2::kosong::protocol::protocol_trait::{
    ProtocolEndpoint, ProtocolTrait, TraitContext,
};

use super::config::ModelSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostHeaders {
    Full,
    UserAgent,
}

#[derive(Clone)]
pub struct ProviderDefinition {
    pub id: String,
    pub base_protocol: Protocol,
    pub traits: Vec<Arc<ProtocolTrait>>,
    pub endpoint: Option<ProtocolEndpoint>,
    pub host_headers: Option<HostHeaders>,
    pub model_source: Option<ModelSource>,
    pub capability: Option<ModelCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderDefinitionRegistryError {
    AlreadyRegistered { id: String, protocol: Protocol },
    Poisoned,
}

impl fmt::Display for ProviderDefinitionRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered { id, protocol } => write!(
                formatter,
                "provider definition '{id}' is already registered for protocol '{protocol}'"
            ),
            Self::Poisoned => formatter.write_str("provider definition registry lock is poisoned"),
        }
    }
}

impl Error for ProviderDefinitionRegistryError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedProviderEndpoint {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExplainedProviderEndpoint {
    pub api_key: Option<String>,
    pub api_key_env_name: Option<String>,
    pub base_url: Option<String>,
    pub base_url_env_name: Option<String>,
    pub base_url_is_default: Option<bool>,
}

#[derive(Default)]
pub struct ProviderDefinitionRegistry {
    definitions: IndexMap<String, IndexMap<Protocol, Arc<ProviderDefinition>>>,
}

impl ProviderDefinitionRegistry {
    // Original: providerDefinition.ts, registerProviderDefinition()
    pub fn register(
        &mut self,
        definition: ProviderDefinition,
    ) -> Result<(), ProviderDefinitionRegistryError> {
        let by_protocol = self.definitions.entry(definition.id.clone()).or_default();
        if by_protocol.contains_key(&definition.base_protocol) {
            return Err(ProviderDefinitionRegistryError::AlreadyRegistered {
                id: definition.id,
                protocol: definition.base_protocol,
            });
        }
        by_protocol.insert(definition.base_protocol, Arc::new(definition));
        Ok(())
    }

    // Original: providerDefinition.ts, getProviderDefinition()
    pub fn get(&self, id: &str, protocol: Option<Protocol>) -> Option<Arc<ProviderDefinition>> {
        let by_protocol = self.definitions.get(id)?;
        match protocol {
            Some(protocol) => by_protocol.get(&protocol).cloned(),
            None => by_protocol
                .first()
                .map(|(_, definition)| Arc::clone(definition)),
        }
    }

    pub fn get_all(&self, id: &str) -> Vec<Arc<ProviderDefinition>> {
        self.definitions
            .get(id)
            .map_or_else(Vec::new, |definitions| {
                definitions.values().cloned().collect()
            })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.definitions.contains_key(id)
    }

    pub fn is_oauth_catalog_vendor(&self, id: Option<&str>) -> bool {
        id.is_some_and(|id| {
            self.get_all(id)
                .iter()
                .any(|definition| definition.model_source == Some(ModelSource::OAuthCatalog))
        })
    }

    pub fn list(&self) -> Vec<Arc<ProviderDefinition>> {
        self.definitions
            .values()
            .flat_map(|definitions| definitions.values().cloned())
            .collect()
    }

    // Original: providerDefinition.ts, explainProviderEndpoint()
    pub fn explain_endpoint(
        &self,
        provider_type: &str,
        env: &IndexMap<String, String>,
    ) -> ExplainedProviderEndpoint {
        let Some(definition) = self.get(provider_type, None) else {
            return ExplainedProviderEndpoint::default();
        };
        let endpoint = definition
            .endpoint
            .as_ref()
            .map(normalize_endpoint_declaration)
            .or_else(|| aggregate_trait_endpoints(&definition));
        let Some(endpoint) = endpoint else {
            return ExplainedProviderEndpoint::default();
        };
        let api_key_hit = first_env_hit(&endpoint.api_key_env, env);
        let base_url_hit = first_env_hit(&endpoint.base_url_env, env);
        ExplainedProviderEndpoint {
            api_key: api_key_hit.as_ref().map(|hit| hit.value.clone()),
            api_key_env_name: api_key_hit.map(|hit| hit.name),
            base_url: base_url_hit
                .as_ref()
                .map(|hit| hit.value.clone())
                .or(endpoint.default_base_url.clone()),
            base_url_env_name: base_url_hit.as_ref().map(|hit| hit.name.clone()),
            base_url_is_default: if base_url_hit.is_none() && endpoint.default_base_url.is_some() {
                Some(true)
            } else {
                None
            },
        }
    }

    pub fn resolve_endpoint(
        &self,
        provider_type: &str,
        env: &IndexMap<String, String>,
    ) -> ResolvedProviderEndpoint {
        let explained = self.explain_endpoint(provider_type, env);
        ResolvedProviderEndpoint {
            api_key: explained.api_key,
            base_url: explained.base_url,
        }
    }
}

#[derive(Debug)]
struct AggregatedEndpointDeclaration {
    api_key_env: Vec<String>,
    base_url_env: Vec<String>,
    default_base_url: Option<String>,
}

fn normalize_endpoint_declaration(endpoint: &ProtocolEndpoint) -> AggregatedEndpointDeclaration {
    AggregatedEndpointDeclaration {
        api_key_env: endpoint.api_key_env.iter().cloned().collect(),
        base_url_env: endpoint.base_url_env.iter().cloned().collect(),
        default_base_url: endpoint.default_base_url.clone(),
    }
}

// Original: providerDefinition.ts, aggregateTraitEndpoints()
fn aggregate_trait_endpoints(
    definition: &ProviderDefinition,
) -> Option<AggregatedEndpointDeclaration> {
    let config = ProtocolAdapterConfig {
        protocol: definition.base_protocol,
        provider_type: Some(definition.id.clone()),
        base_url: None,
        model_name: String::new(),
        api_key: None,
        default_headers: None,
        provider_options: None,
    };
    let context = TraitContext {
        config,
        provider_id: Some(definition.id.clone()),
    };
    let mut aggregated = AggregatedEndpointDeclaration {
        api_key_env: Vec::new(),
        base_url_env: Vec::new(),
        default_base_url: None,
    };
    let mut declared = false;
    for protocol_trait in &definition.traits {
        let Some(hook) = protocol_trait.endpoint.as_ref() else {
            continue;
        };
        let Some(endpoint) = hook(&context) else {
            continue;
        };
        declared = true;
        aggregated.api_key_env.extend(endpoint.api_key_env);
        aggregated.base_url_env.extend(endpoint.base_url_env);
        if endpoint.default_base_url.is_some() {
            aggregated.default_base_url = endpoint.default_base_url;
        }
    }
    declared.then_some(aggregated)
}

struct EnvHit {
    name: String,
    value: String,
}

fn first_env_hit(names: &[String], env: &IndexMap<String, String>) -> Option<EnvHit> {
    names.iter().find_map(|name| {
        env.get(name)
            .filter(|value| !value.is_empty())
            .map(|value| EnvHit {
                name: name.clone(),
                value: value.clone(),
            })
    })
}

static PROVIDER_DEFINITIONS: LazyLock<RwLock<ProviderDefinitionRegistry>> =
    LazyLock::new(|| RwLock::new(ProviderDefinitionRegistry::default()));

pub fn register_provider_definition(
    definition: ProviderDefinition,
) -> Result<(), ProviderDefinitionRegistryError> {
    PROVIDER_DEFINITIONS
        .write()
        .map_err(|_| ProviderDefinitionRegistryError::Poisoned)?
        .register(definition)
}

pub fn get_provider_definition(
    id: &str,
    protocol: Option<Protocol>,
) -> Result<Option<Arc<ProviderDefinition>>, ProviderDefinitionRegistryError> {
    Ok(PROVIDER_DEFINITIONS
        .read()
        .map_err(|_| ProviderDefinitionRegistryError::Poisoned)?
        .get(id, protocol))
}

pub fn list_provider_definitions()
-> Result<Vec<Arc<ProviderDefinition>>, ProviderDefinitionRegistryError> {
    Ok(PROVIDER_DEFINITIONS
        .read()
        .map_err(|_| ProviderDefinitionRegistryError::Poisoned)?
        .list())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_core_v2::kosong::contract::capability::UNKNOWN_CAPABILITY;

    fn definition(id: &str, protocol: Protocol) -> ProviderDefinition {
        ProviderDefinition {
            id: id.to_owned(),
            base_protocol: protocol,
            traits: Vec::new(),
            endpoint: None,
            host_headers: None,
            model_source: None,
            capability: None,
        }
    }

    #[test]
    fn pair_registration_and_queries_preserve_both_order_levels() {
        let mut registry = ProviderDefinitionRegistry::default();
        registry
            .register(definition("kimi", Protocol::OpenAi))
            .unwrap();
        registry
            .register(definition("kimi", Protocol::Anthropic))
            .unwrap();
        registry
            .register(definition("other", Protocol::GoogleGenAi))
            .unwrap();
        assert_eq!(
            registry.get("kimi", None).unwrap().base_protocol,
            Protocol::OpenAi
        );
        assert_eq!(registry.get_all("kimi").len(), 2);
        assert!(registry.contains("kimi"));
        assert!(!registry.contains("missing"));
        assert_eq!(
            registry
                .list()
                .iter()
                .map(|item| (&item.id, item.base_protocol))
                .collect::<Vec<_>>(),
            vec![
                (&"kimi".to_owned(), Protocol::OpenAi),
                (&"kimi".to_owned(), Protocol::Anthropic),
                (&"other".to_owned(), Protocol::GoogleGenAi),
            ]
        );
    }

    #[test]
    fn duplicate_pair_is_rejected_without_rejecting_another_protocol() {
        let mut registry = ProviderDefinitionRegistry::default();
        registry
            .register(definition("vendor", Protocol::OpenAi))
            .unwrap();
        registry
            .register(definition("vendor", Protocol::Anthropic))
            .unwrap();
        let error = registry
            .register(definition("vendor", Protocol::OpenAi))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "provider definition 'vendor' is already registered for protocol 'openai'"
        );
        assert_eq!(registry.get_all("vendor").len(), 2);
    }

    #[test]
    fn oauth_catalog_is_a_declared_fact_not_an_id_comparison() {
        let mut registry = ProviderDefinitionRegistry::default();
        let mut configured = definition("external", Protocol::OpenAi);
        configured.model_source = Some(ModelSource::OAuthCatalog);
        configured.capability = Some(UNKNOWN_CAPABILITY);
        registry.register(configured).unwrap();
        assert!(registry.is_oauth_catalog_vendor(Some("external")));
        assert!(!registry.is_oauth_catalog_vendor(Some("kimi")));
        assert!(!registry.is_oauth_catalog_vendor(None));
    }

    #[test]
    fn definition_endpoint_uses_first_nonempty_env_then_default_url() {
        let mut registry = ProviderDefinitionRegistry::default();
        let mut configured = definition("kimi", Protocol::OpenAi);
        configured.endpoint = Some(ProtocolEndpoint {
            api_key_env: Some("KIMI_API_KEY".to_owned()),
            base_url_env: Some("KIMI_BASE_URL".to_owned()),
            default_base_url: Some("https://api.moonshot.ai/v1".to_owned()),
        });
        registry.register(configured).unwrap();
        assert_eq!(
            registry.explain_endpoint(
                "kimi",
                &IndexMap::from([("KIMI_API_KEY".to_owned(), "sk-env".to_owned())])
            ),
            ExplainedProviderEndpoint {
                api_key: Some("sk-env".to_owned()),
                api_key_env_name: Some("KIMI_API_KEY".to_owned()),
                base_url: Some("https://api.moonshot.ai/v1".to_owned()),
                base_url_env_name: None,
                base_url_is_default: Some(true),
            }
        );
        assert_eq!(
            registry.resolve_endpoint("missing", &IndexMap::new()),
            ResolvedProviderEndpoint::default()
        );
    }

    #[test]
    fn trait_endpoints_aggregate_in_order_with_last_default_winning() {
        let mut configured = definition("google", Protocol::GoogleGenAi);
        configured.traits = vec![
            Arc::new(ProtocolTrait {
                endpoint: Some(Arc::new(|_| {
                    Some(ProtocolEndpoint {
                        api_key_env: Some("VERTEXAI_API_KEY".to_owned()),
                        base_url_env: Some("VERTEX_BASE_URL".to_owned()),
                        default_base_url: Some("https://first".to_owned()),
                    })
                })),
                ..ProtocolTrait::default()
            }),
            Arc::new(ProtocolTrait {
                endpoint: Some(Arc::new(|context| {
                    assert_eq!(context.provider_id.as_deref(), Some("google"));
                    Some(ProtocolEndpoint {
                        api_key_env: Some("GOOGLE_API_KEY".to_owned()),
                        base_url_env: Some("GEMINI_BASE_URL".to_owned()),
                        default_base_url: Some("https://last".to_owned()),
                    })
                })),
                ..ProtocolTrait::default()
            }),
        ];
        let mut registry = ProviderDefinitionRegistry::default();
        registry.register(configured).unwrap();
        let env = IndexMap::from([
            ("VERTEXAI_API_KEY".to_owned(), String::new()),
            ("GOOGLE_API_KEY".to_owned(), "google-key".to_owned()),
            ("GEMINI_BASE_URL".to_owned(), "https://env".to_owned()),
        ]);
        assert_eq!(
            registry.resolve_endpoint("google", &env),
            ResolvedProviderEndpoint {
                api_key: Some("google-key".to_owned()),
                base_url: Some("https://env".to_owned()),
            }
        );
        assert_eq!(
            registry
                .resolve_endpoint("google", &IndexMap::new())
                .base_url
                .as_deref(),
            Some("https://last")
        );
    }
}
