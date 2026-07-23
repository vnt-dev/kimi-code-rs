use std::sync::{Arc, LazyLock, RwLock};

use crate::_base::di::{
    descriptors::SyncDescriptor,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};
use crate::kosong::contract::capability::{ModelCapability, UNKNOWN_CAPABILITY};
use crate::kosong::contract::errors::ChatProviderError;
use crate::kosong::contract::inspection::{InspectionSource, InspectionSourceKind};
use crate::kosong::contract::provider::{ChatProvider, ProviderError};
use crate::kosong::protocol::identity::{
    ExplainedCapability, PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID, Protocol, ProtocolAdapterConfig,
    ProtocolAdapterRegistry as ProtocolAdapterRegistryContract, ProtocolAdapterRegistryHandle,
};
use crate::kosong::protocol::protocol_base::{
    PROTOCOL_BASES, ProtocolBaseContext, ProtocolBaseId, ProtocolBaseRegistry,
    ResolvedAdapterIdentity,
};
use crate::kosong::protocol::protocol_trait::{ProtocolTrait, ResolvedTrait, TraitContext};

use super::provider_definition::{PROVIDER_DEFINITIONS, ProviderDefinitionRegistry};

static CONFIG_DEFAULT_HEADERS_TRAIT: LazyLock<Arc<ProtocolTrait>> = LazyLock::new(|| {
    Arc::new(ProtocolTrait {
        default_headers: Some(Arc::new(|context| context.config.default_headers.clone())),
        ..ProtocolTrait::default()
    })
});

// Original:
//   packages/agent-core-v2/src/kosong/provider/protocolAdapterRegistry.ts
//   ProtocolAdapterRegistry
//
// Rust adaptation:
//   Registry references are injected so tests and embedders can isolate
//   registration state. new() binds the original process-wide registries.
pub struct ProtocolAdapterRegistry<'a> {
    provider_definitions: &'a RwLock<ProviderDefinitionRegistry>,
    protocol_bases: &'a RwLock<ProtocolBaseRegistry>,
}

impl ProtocolAdapterRegistry<'static> {
    pub fn new() -> Self {
        Self::with_registries(&PROVIDER_DEFINITIONS, &PROTOCOL_BASES)
    }
}

impl Default for ProtocolAdapterRegistry<'static> {
    fn default() -> Self {
        Self::new()
    }
}

// Original: protocolAdapterRegistry.ts, eager app-scoped service registration.
pub fn register_protocol_adapter_registry() {
    register_scoped_service(
        LifecycleScope::App,
        PROTOCOL_ADAPTER_REGISTRY_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let registry: Arc<dyn ProtocolAdapterRegistryContract> =
                Arc::new(ProtocolAdapterRegistry::new());
            Ok(ProtocolAdapterRegistryHandle(registry))
        }),
        InstantiationType::Eager,
        "provider",
    );
}

impl<'a> ProtocolAdapterRegistry<'a> {
    pub fn with_registries(
        provider_definitions: &'a RwLock<ProviderDefinitionRegistry>,
        protocol_bases: &'a RwLock<ProtocolBaseRegistry>,
    ) -> Self {
        Self {
            provider_definitions,
            protocol_bases,
        }
    }

    fn definition(
        &self,
        provider_type: Option<&str>,
        protocol: Protocol,
    ) -> Option<Arc<super::provider_definition::ProviderDefinition>> {
        provider_type.and_then(|provider_type| {
            self.provider_definitions
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(provider_type, Some(protocol))
        })
    }
}

impl ProtocolAdapterRegistryContract for ProtocolAdapterRegistry<'_> {
    fn supported_protocols(&self) -> Vec<Protocol> {
        self.protocol_bases
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .list()
            .iter()
            .map(|base| base.id)
            .collect()
    }

    fn resolve_adapter_identity(
        &self,
        protocol: Protocol,
        provider_type: Option<&str>,
    ) -> ResolvedAdapterIdentity {
        let definition = self.definition(provider_type, protocol);
        let context = TraitContext {
            config: ProtocolAdapterConfig {
                protocol,
                provider_type: provider_type.map(str::to_owned),
                base_url: None,
                model_name: String::new(),
                api_key: None,
                default_headers: None,
                provider_options: None,
            },
            provider_id: provider_type.map(str::to_owned),
        };
        let mut traits = definition.map_or_else(Vec::new, |definition| {
            definition
                .traits
                .iter()
                .map(|protocol_trait| ResolvedTrait {
                    protocol_trait: Arc::clone(protocol_trait),
                    context: context.clone(),
                })
                .collect()
        });
        traits.push(ResolvedTrait {
            protocol_trait: Arc::clone(&CONFIG_DEFAULT_HEADERS_TRAIT),
            context,
        });
        ResolvedAdapterIdentity {
            base_id: protocol,
            traits,
        }
    }

    fn resolve_provider_base_id(
        &self,
        protocol: Protocol,
        provider_type: Option<&str>,
    ) -> ProtocolBaseId {
        self.definition(provider_type, protocol)
            .map_or(protocol, |definition| definition.base_protocol)
    }

    fn resolve_capability(
        &self,
        protocol: Protocol,
        model_name: &str,
        provider_type: Option<&str>,
    ) -> ModelCapability {
        self.explain_capability(protocol, model_name, provider_type)
            .capability
    }

    fn explain_capability(
        &self,
        protocol: Protocol,
        model_name: &str,
        provider_type: Option<&str>,
    ) -> ExplainedCapability {
        if let Some(capability) = self
            .definition(provider_type, protocol)
            .and_then(|definition| definition.capability.clone())
        {
            return ExplainedCapability {
                capability,
                source: InspectionSource {
                    kind: InspectionSourceKind::Builtin,
                    detail: Some(format!(
                        "provider definition '{}' (pair with protocol '{protocol}')",
                        provider_type.unwrap_or("unregistered")
                    )),
                },
            };
        }
        let identity = self.resolve_adapter_identity(protocol, provider_type);
        let mut trait_capability = None;
        for resolved in &identity.traits {
            if let Some(hook) = resolved.protocol_trait.capability.as_ref()
                && let Some(capability) = hook(model_name, &resolved.context)
            {
                trait_capability = Some(capability);
            }
        }
        if let Some(capability) = trait_capability {
            return ExplainedCapability {
                capability,
                source: InspectionSource {
                    kind: InspectionSourceKind::Builtin,
                    detail: Some(format!(
                        "trait capability hook (provider '{}')",
                        provider_type.unwrap_or("unregistered")
                    )),
                },
            };
        }
        let base_capability = self
            .protocol_bases
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity.base_id)
            .and_then(|base| base.capability.as_ref().and_then(|hook| hook(model_name)));
        base_capability.map_or_else(
            || ExplainedCapability {
                capability: UNKNOWN_CAPABILITY,
                source: InspectionSource {
                    kind: InspectionSourceKind::None,
                    detail: Some("no capability source knew this model".to_owned()),
                },
            },
            |capability| ExplainedCapability {
                capability,
                source: InspectionSource {
                    kind: InspectionSourceKind::Builtin,
                    detail: Some(format!("protocol base '{}' catalog", identity.base_id)),
                },
            },
        )
    }

    fn create_chat_provider(
        &self,
        config: ProtocolAdapterConfig,
    ) -> Result<Arc<dyn ChatProvider>, ProviderError> {
        let identity =
            self.resolve_adapter_identity(config.protocol, config.provider_type.as_deref());
        let traits = identity
            .traits
            .iter()
            .map(|resolved| ResolvedTrait {
                protocol_trait: Arc::clone(&resolved.protocol_trait),
                context: TraitContext {
                    config: config.clone(),
                    provider_id: config.provider_type.clone(),
                },
            })
            .collect();
        let base = self
            .protocol_bases
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity.base_id)
            .ok_or_else(|| {
                Box::new(ChatProviderError::ChatProvider {
                    message: format!(
                        "No protocol base registered for '{}'. Import the base's contrib module first.",
                        identity.base_id
                    ),
                }) as ProviderError
            })?;
        (base.create_chat_provider)(&ProtocolBaseContext { config, traits })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::protocol::protocol_base::ProtocolBaseDefinition;
    use crate::kosong::provider::provider_definition::ProviderDefinition;
    use indexmap::IndexMap;
    use std::io;
    use std::sync::Mutex;

    fn registries() -> (
        RwLock<ProviderDefinitionRegistry>,
        RwLock<ProtocolBaseRegistry>,
    ) {
        (
            RwLock::new(ProviderDefinitionRegistry::default()),
            RwLock::new(ProtocolBaseRegistry::default()),
        )
    }

    #[test]
    fn identity_uses_exact_pair_traits_and_always_appends_config_headers() {
        let (definitions, bases) = registries();
        definitions
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "vendor".to_owned(),
                base_protocol: Protocol::OpenAi,
                traits: vec![Arc::new(ProtocolTrait::default())],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        let registry = ProtocolAdapterRegistry::with_registries(&definitions, &bases);
        assert_eq!(
            registry
                .resolve_adapter_identity(Protocol::OpenAi, Some("vendor"))
                .traits
                .len(),
            2
        );
        assert_eq!(
            registry
                .resolve_adapter_identity(Protocol::Anthropic, Some("vendor"))
                .traits
                .len(),
            1
        );
        let mut identity = registry.resolve_adapter_identity(Protocol::OpenAi, None);
        identity
            .traits
            .last_mut()
            .unwrap()
            .context
            .config
            .default_headers = Some(IndexMap::from([("x-test".to_owned(), "config".to_owned())]));
        assert_eq!(
            (identity
                .traits
                .last()
                .unwrap()
                .protocol_trait
                .default_headers
                .as_ref()
                .unwrap())(&identity.traits.last().unwrap().context),
            Some(IndexMap::from([("x-test".to_owned(), "config".to_owned())]))
        );
    }

    #[test]
    fn capability_fallback_is_definition_then_last_trait_then_base_then_unknown() {
        let (definitions, bases) = registries();
        let declared = ModelCapability {
            image_in: true,
            ..UNKNOWN_CAPABILITY
        };
        definitions
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "vendor".to_owned(),
                base_protocol: Protocol::OpenAi,
                traits: vec![Arc::new(ProtocolTrait {
                    capability: Some(Arc::new(|_, _| {
                        Some(ModelCapability {
                            thinking: true,
                            ..UNKNOWN_CAPABILITY
                        })
                    })),
                    ..ProtocolTrait::default()
                })],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: Some(declared.clone()),
            })
            .unwrap();
        definitions
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "trait-vendor".to_owned(),
                base_protocol: Protocol::OpenAi,
                traits: vec![
                    Arc::new(ProtocolTrait {
                        capability: Some(Arc::new(|_, _| {
                            Some(ModelCapability {
                                image_in: true,
                                ..UNKNOWN_CAPABILITY
                            })
                        })),
                        ..ProtocolTrait::default()
                    }),
                    Arc::new(ProtocolTrait {
                        capability: Some(Arc::new(|_, _| {
                            Some(ModelCapability {
                                thinking: true,
                                ..UNKNOWN_CAPABILITY
                            })
                        })),
                        ..ProtocolTrait::default()
                    }),
                ],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        bases
            .write()
            .unwrap()
            .register(ProtocolBaseDefinition {
                id: Protocol::OpenAi,
                capability: Some(Arc::new(|_| {
                    Some(ModelCapability {
                        tool_use: true,
                        ..UNKNOWN_CAPABILITY
                    })
                })),
                create_chat_provider: Arc::new(|_| unreachable!()),
            })
            .unwrap();
        let registry = ProtocolAdapterRegistry::with_registries(&definitions, &bases);
        assert_eq!(
            registry.resolve_capability(Protocol::OpenAi, "m", Some("vendor")),
            declared
        );
        let trait_capability =
            registry.resolve_capability(Protocol::OpenAi, "m", Some("trait-vendor"));
        assert!(trait_capability.thinking);
        assert!(!trait_capability.image_in);
        assert!(
            registry
                .resolve_capability(Protocol::OpenAi, "m", None)
                .tool_use
        );
        assert_eq!(
            registry.resolve_capability(Protocol::Anthropic, "m", None),
            UNKNOWN_CAPABILITY
        );
    }

    #[test]
    fn provider_creation_rebinds_traits_to_the_full_config_before_delegating() {
        let (definitions, bases) = registries();
        definitions
            .write()
            .unwrap()
            .register(ProviderDefinition {
                id: "vendor".to_owned(),
                base_protocol: Protocol::OpenAi,
                traits: vec![Arc::new(ProtocolTrait::default())],
                endpoint: None,
                host_headers: None,
                model_source: None,
                capability: None,
            })
            .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_factory = Arc::clone(&seen);
        bases
            .write()
            .unwrap()
            .register(ProtocolBaseDefinition {
                id: Protocol::OpenAi,
                capability: None,
                create_chat_provider: Arc::new(move |context| {
                    seen_by_factory.lock().unwrap().push((
                        context.config.model_name.clone(),
                        context
                            .traits
                            .iter()
                            .map(|resolved| resolved.context.config.model_name.clone())
                            .collect::<Vec<_>>(),
                    ));
                    Err(Box::new(io::Error::other("factory called")))
                }),
            })
            .unwrap();
        let registry = ProtocolAdapterRegistry::with_registries(&definitions, &bases);
        let result = registry.create_chat_provider(ProtocolAdapterConfig {
            protocol: Protocol::OpenAi,
            provider_type: Some("vendor".to_owned()),
            base_url: None,
            model_name: "real-model".to_owned(),
            api_key: None,
            default_headers: Some(IndexMap::from([("x-test".to_owned(), "config".to_owned())])),
            provider_options: None,
        });
        assert_eq!(result.err().unwrap().to_string(), "factory called");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![(
                "real-model".to_owned(),
                vec!["real-model".to_owned(), "real-model".to_owned()]
            )]
        );
    }

    #[test]
    fn missing_base_returns_the_original_provider_error_message() {
        let (definitions, bases) = registries();
        let registry = ProtocolAdapterRegistry::with_registries(&definitions, &bases);
        let error = match registry.create_chat_provider(ProtocolAdapterConfig {
            protocol: Protocol::OpenAi,
            provider_type: None,
            base_url: None,
            model_name: "m".to_owned(),
            api_key: None,
            default_headers: None,
            provider_options: None,
        }) {
            Ok(_) => panic!("missing base must fail"),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "No protocol base registered for 'openai'. Import the base's contrib module first."
        );
    }
}
