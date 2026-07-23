//! Process-level agent-file discovery overrides.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentCatalogRuntimeOptions.ts`.

use std::sync::Arc;

use crate::_base::di::{
    descriptors::SyncDescriptor,
    instantiation::ServiceIdentifier,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
    service_collection::ServiceCollection,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentCatalogRuntimeOptions {
    pub explicit_files: Option<Vec<String>>,
}

impl AgentCatalogRuntimeOptions {
    // Original: AgentCatalogRuntimeOptions.constructor().
    pub fn new(explicit_files: Option<Vec<String>>) -> Self {
        Self { explicit_files }
    }
}

pub const AGENT_CATALOG_RUNTIME_OPTIONS_ID: ServiceIdentifier<AgentCatalogRuntimeOptions> =
    ServiceIdentifier::new("agentCatalogRuntimeOptions");

// Original: agentCatalogRuntimeOptionsSeed(). An absent or empty CLI list does
// not shadow the registered App-scope default.
pub fn agent_catalog_runtime_options_seed(
    explicit_files: Option<Vec<String>>,
) -> ServiceCollection {
    let mut seed = ServiceCollection::new();
    if explicit_files
        .as_ref()
        .is_some_and(|files| !files.is_empty())
    {
        seed.set_instance(
            AGENT_CATALOG_RUNTIME_OPTIONS_ID,
            Arc::new(AgentCatalogRuntimeOptions::new(explicit_files)),
        );
    }
    seed
}

pub fn register_agent_catalog_runtime_options() {
    register_scoped_service(
        LifecycleScope::App,
        AGENT_CATALOG_RUNTIME_OPTIONS_ID,
        SyncDescriptor::new(|_| Ok(AgentCatalogRuntimeOptions::default())),
        InstantiationType::Eager,
        "agentFileCatalog",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_only_contains_nonempty_explicit_file_overrides() {
        assert!(
            agent_catalog_runtime_options_seed(None)
                .get(AGENT_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .is_none()
        );
        assert!(
            agent_catalog_runtime_options_seed(Some(Vec::new()))
                .get(AGENT_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .is_none()
        );

        let seed = agent_catalog_runtime_options_seed(Some(vec!["agent.md".into()]));
        assert_eq!(
            seed.get(AGENT_CATALOG_RUNTIME_OPTIONS_ID)
                .unwrap()
                .as_deref(),
            Some(&AgentCatalogRuntimeOptions {
                explicit_files: Some(vec!["agent.md".into()]),
            })
        );
        assert_eq!(
            AGENT_CATALOG_RUNTIME_OPTIONS_ID.to_string(),
            "agentCatalogRuntimeOptions"
        );
    }
}
