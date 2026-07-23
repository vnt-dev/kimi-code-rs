use std::{ops::Deref, sync::Arc};

use crate::_base::di::{
    descriptors::SyncDescriptor,
    errors::DiError,
    instantiation::{ServiceIdentifier, ServicesAccessor, ServicesAccessorExt},
    lifecycle::{Disposable, DisposableStore, DisposeResult},
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::{
    AGENT_TOOL_REGISTRY_SERVICE_ID, AgentToolRegistryServiceHandle, ToolRegistrationOptions,
    get_tool_contributions,
};

pub trait AgentBuiltinToolsRegistrarContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentBuiltinToolsRegistrarHandle(pub Arc<dyn AgentBuiltinToolsRegistrarContract>);

impl Deref for AgentBuiltinToolsRegistrarHandle {
    type Target = dyn AgentBuiltinToolsRegistrarContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentBuiltinToolsRegistrarHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_BUILTIN_TOOLS_REGISTRAR_ID: ServiceIdentifier<AgentBuiltinToolsRegistrarHandle> =
    ServiceIdentifier::new("agentBuiltinToolsRegistrar");

pub struct AgentBuiltinToolsRegistrar {
    registrations: DisposableStore,
}

impl AgentBuiltinToolsRegistrar {
    // Original: builtinToolsRegistrar.ts, constructor(). Contributions are
    // instantiated only after the registry itself has finished construction.
    pub fn new(
        accessor: &dyn ServicesAccessor,
        registry: &AgentToolRegistryServiceHandle,
    ) -> Result<Self, DiError> {
        let registrations = DisposableStore::new();
        for contribution in get_tool_contributions() {
            if contribution
                .options
                .when
                .as_ref()
                .is_some_and(|condition| !condition(accessor))
            {
                continue;
            }
            let tool = (contribution.factory)(accessor)?;
            registrations.add(registry.register(
                tool,
                ToolRegistrationOptions {
                    source: contribution.options.source,
                },
            ));
        }
        Ok(Self { registrations })
    }
}

impl Disposable for AgentBuiltinToolsRegistrar {
    fn dispose(&self) -> DisposeResult {
        self.registrations.dispose()
    }
}

impl AgentBuiltinToolsRegistrarContract for AgentBuiltinToolsRegistrar {}

// Original: builtinToolsRegistrar.ts, eager Agent-scope registration.
pub fn register_agent_builtin_tools_registrar() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_BUILTIN_TOOLS_REGISTRAR_ID,
        SyncDescriptor::new(|accessor| {
            let registry = accessor.get(AGENT_TOOL_REGISTRY_SERVICE_ID)?;
            let registrar: Arc<dyn AgentBuiltinToolsRegistrarContract> = Arc::new(
                AgentBuiltinToolsRegistrar::new(accessor, registry.as_ref())?,
            );
            Ok(AgentBuiltinToolsRegistrarHandle(registrar))
        })
        .disposable(),
        InstantiationType::Eager,
        "toolRegistry",
    );
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::di::{
            errors::DiError,
            instantiation::{ErasedServiceIdentifier, ServicesAccessor},
            service_collection::ServiceValue,
        },
        agent::tool_registry::{
            AgentToolRegistryService, AgentToolRegistryServiceContract, ToolContributionOptions,
            clear_tool_contributions_for_tests, register_tool,
        },
        kosong::contract::tool::Tool,
        tool::{ExecutableTool, ToolExecution, ToolSource},
    };

    struct EmptyAccessor;

    impl ServicesAccessor for EmptyAccessor {
        fn get_erased(&self, id: ErasedServiceIdentifier) -> Result<ServiceValue, DiError> {
            Err(DiError::UnknownService(id))
        }
    }

    struct TestTool(Tool);

    #[async_trait]
    impl ExecutableTool for TestTool {
        type Input = Value;

        fn tool(&self) -> &Tool {
            &self.0
        }

        async fn resolve_execution(&self, _input: Value) -> ToolExecution {
            ToolExecution::Error(crate::tool::ExecutableToolResult::success("unused"))
        }
    }

    fn factory(name: &'static str) -> super::super::ToolFactory {
        Arc::new(move |_accessor| {
            Ok(Arc::new(TestTool(Tool {
                name: name.into(),
                description: name.into(),
                parameters: serde_json::Map::new(),
                deferred: None,
            })))
        })
    }

    #[test]
    fn consumes_matching_contributions_and_disposes_registrations() {
        clear_tool_contributions_for_tests();
        register_tool(factory("Enabled"), ToolContributionOptions::default());
        register_tool(
            factory("Disabled"),
            ToolContributionOptions {
                source: Some(ToolSource::User),
                when: Some(Arc::new(|_accessor| false)),
            },
        );
        let registry = Arc::new(AgentToolRegistryService::new());
        let registry_contract: Arc<dyn AgentToolRegistryServiceContract> = registry.clone();
        let handle = AgentToolRegistryServiceHandle(registry_contract);
        let registrar = AgentBuiltinToolsRegistrar::new(&EmptyAccessor, &handle).unwrap();

        assert!(registry.resolve("Enabled").is_some());
        assert!(registry.resolve("Disabled").is_none());
        registrar.dispose().unwrap();
        assert!(registry.list().is_empty());
        clear_tool_contributions_for_tests();
    }

    #[test]
    fn registrar_identifier_matches_source() {
        assert_eq!(
            AGENT_BUILTIN_TOOLS_REGISTRAR_ID.to_string(),
            "agentBuiltinToolsRegistrar"
        );
    }
}
