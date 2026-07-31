use indexmap::IndexMap;
use std::error::Error;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};

use crate::kosong::contract::capability::ModelCapability;
use crate::kosong::contract::provider::{ChatProvider, ProviderError};

use super::identity::{Protocol, ProtocolAdapterConfig};
use super::protocol_trait::ResolvedTrait;

pub type ProtocolBaseId = Protocol;

#[derive(Clone)]
pub struct ProtocolBaseContext {
    pub config: ProtocolAdapterConfig,
    pub traits: Vec<ResolvedTrait>,
}

pub type BaseCapabilityHook = Arc<dyn Fn(&str) -> Option<ModelCapability> + Send + Sync>;
pub type CreateChatProviderHook =
    Arc<dyn Fn(&ProtocolBaseContext) -> Result<Arc<dyn ChatProvider>, ProviderError> + Send + Sync>;

#[derive(Clone)]
pub struct ProtocolBaseDefinition {
    pub id: ProtocolBaseId,
    pub capability: Option<BaseCapabilityHook>,
    pub create_chat_provider: CreateChatProviderHook,
}

#[derive(Clone)]
pub struct ResolvedAdapterIdentity {
    pub base_id: ProtocolBaseId,
    pub traits: Vec<ResolvedTrait>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolBaseRegistryError {
    AlreadyRegistered(ProtocolBaseId),
    Poisoned,
}

impl fmt::Display for ProtocolBaseRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRegistered(id) => {
                write!(formatter, "protocol base '{id}' is already registered")
            }
            Self::Poisoned => formatter.write_str("protocol base registry lock is poisoned"),
        }
    }
}

impl Error for ProtocolBaseRegistryError {}

// Original:
//   packages/agent-core-v2/src/kosong/protocol/protocolBase.ts
//   protocolBases / registerProtocolBase() / getProtocolBase() /
//   listProtocolBases()
//
// Rust adaptation:
//   IndexMap preserves JavaScript Map registration order. A small registry
//   value makes the state machine independently testable; the public module
//   functions below retain the original process-wide singleton behavior.
#[derive(Default)]
pub struct ProtocolBaseRegistry {
    bases: IndexMap<ProtocolBaseId, Arc<ProtocolBaseDefinition>>,
}

impl ProtocolBaseRegistry {
    pub fn register(
        &mut self,
        definition: ProtocolBaseDefinition,
    ) -> Result<(), ProtocolBaseRegistryError> {
        if self.bases.contains_key(&definition.id) {
            return Err(ProtocolBaseRegistryError::AlreadyRegistered(definition.id));
        }
        self.bases.insert(definition.id, Arc::new(definition));
        Ok(())
    }

    pub fn get(&self, id: ProtocolBaseId) -> Option<Arc<ProtocolBaseDefinition>> {
        self.bases.get(&id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<ProtocolBaseDefinition>> {
        self.bases.values().cloned().collect()
    }
}

pub(crate) static PROTOCOL_BASES: LazyLock<RwLock<ProtocolBaseRegistry>> =
    LazyLock::new(|| RwLock::new(ProtocolBaseRegistry::default()));

pub fn register_protocol_base(
    definition: ProtocolBaseDefinition,
) -> Result<(), ProtocolBaseRegistryError> {
    PROTOCOL_BASES
        .write()
        .map_err(|_| ProtocolBaseRegistryError::Poisoned)?
        .register(definition)
}

pub fn get_protocol_base(
    id: ProtocolBaseId,
) -> Result<Option<Arc<ProtocolBaseDefinition>>, ProtocolBaseRegistryError> {
    Ok(PROTOCOL_BASES
        .read()
        .map_err(|_| ProtocolBaseRegistryError::Poisoned)?
        .get(id))
}

pub fn list_protocol_bases() -> Result<Vec<Arc<ProtocolBaseDefinition>>, ProtocolBaseRegistryError>
{
    Ok(PROTOCOL_BASES
        .read()
        .map_err(|_| ProtocolBaseRegistryError::Poisoned)?
        .list())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::Message;
    use crate::kosong::contract::provider::{GenerateOptions, StreamedMessage, ThinkingEffort};
    use crate::kosong::contract::tool::Tool;
    use async_trait::async_trait;
    use std::io;
    use std::sync::Mutex;

    struct FakeChatProvider;

    #[async_trait]
    impl ChatProvider for FakeChatProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "test-model"
        }

        fn thinking_effort(&self) -> Option<&ThinkingEffort> {
            None
        }

        fn max_completion_tokens(&self) -> Option<u64> {
            None
        }

        async fn generate(
            &self,
            _system_prompt: &str,
            _tools: &[Tool],
            _history: &[Message],
            _options: Option<&GenerateOptions>,
        ) -> Result<Box<dyn StreamedMessage>, ProviderError> {
            Err(Box::new(io::Error::other("unused")))
        }
    }

    fn config(protocol: Protocol) -> ProtocolAdapterConfig {
        ProtocolAdapterConfig {
            protocol,
            provider_type: None,
            base_url: None,
            model_name: "test-model".to_owned(),
            api_key: None,
            default_headers: None,
            provider_options: None,
        }
    }

    fn fake_base(id: ProtocolBaseId) -> ProtocolBaseDefinition {
        ProtocolBaseDefinition {
            id,
            capability: None,
            create_chat_provider: Arc::new(|_| Ok(Arc::new(FakeChatProvider))),
        }
    }

    #[test]
    fn registration_lookup_and_missing_lookup_match_map_behavior() {
        let mut registry = ProtocolBaseRegistry::default();
        let base = fake_base(Protocol::OpenAi);
        registry.register(base).unwrap();
        assert_eq!(registry.get(Protocol::OpenAi).unwrap().id, Protocol::OpenAi);
        assert!(registry.get(Protocol::OpenAiResponses).is_none());
    }

    #[test]
    fn listing_preserves_registration_order() {
        let mut registry = ProtocolBaseRegistry::default();
        for protocol in [Protocol::OpenAi, Protocol::Anthropic, Protocol::GoogleGenAi] {
            registry.register(fake_base(protocol)).unwrap();
        }
        assert_eq!(
            registry
                .list()
                .iter()
                .map(|base| base.id)
                .collect::<Vec<_>>(),
            vec![Protocol::OpenAi, Protocol::Anthropic, Protocol::GoogleGenAi]
        );
    }

    #[test]
    fn duplicate_registration_returns_exact_source_message_without_overwrite() {
        let mut registry = ProtocolBaseRegistry::default();
        registry.register(fake_base(Protocol::OpenAi)).unwrap();
        let original = registry.get(Protocol::OpenAi).unwrap();
        let error = registry.register(fake_base(Protocol::OpenAi)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "protocol base 'openai' is already registered"
        );
        assert!(Arc::ptr_eq(
            &original,
            &registry.get(Protocol::OpenAi).unwrap()
        ));
    }

    #[test]
    fn provider_factory_receives_the_resolved_context() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_by_factory = Arc::clone(&seen);
        let definition = ProtocolBaseDefinition {
            id: Protocol::OpenAiResponses,
            capability: Some(Arc::new(|_| None)),
            create_chat_provider: Arc::new(move |context| {
                seen_by_factory
                    .lock()
                    .unwrap()
                    .push(context.config.model_name.clone());
                Ok(Arc::new(FakeChatProvider))
            }),
        };
        let mut registry = ProtocolBaseRegistry::default();
        registry.register(definition).unwrap();
        let context = ProtocolBaseContext {
            config: config(Protocol::OpenAi),
            traits: Vec::new(),
        };
        let provider = (registry
            .get(Protocol::OpenAiResponses)
            .unwrap()
            .create_chat_provider)(&context)
        .unwrap();
        assert_eq!(provider.name(), "fake");
        assert_eq!(*seen.lock().unwrap(), vec!["test-model".to_owned()]);
    }
}
