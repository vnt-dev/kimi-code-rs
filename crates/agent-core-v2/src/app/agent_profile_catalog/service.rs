//! App-scoped snapshot of contributed agent profiles.
//!
//! Original: `packages/agent-core-v2/src/app/agentProfileCatalog/agentProfileCatalogService.ts`.

use std::{collections::HashMap, sync::Arc};

use crate::_base::di::{
    descriptors::SyncDescriptor,
    scope::{InstantiationType, LifecycleScope, register_scoped_service},
};

use super::{
    contract::{
        AGENT_PROFILE_CATALOG_SERVICE_ID, AgentProfile, AgentProfileCatalogContract,
        AgentProfileCatalogHandle, DEFAULT_AGENT_PROFILE_NAME, MissingDefaultAgentProfile,
    },
    contribution::get_agent_profile_contributions,
};

pub struct AgentProfileCatalogService {
    by_name: HashMap<String, Arc<AgentProfile>>,
    ordered: Vec<Arc<AgentProfile>>,
}

impl AgentProfileCatalogService {
    // Original: AgentProfileCatalogService.constructor().
    pub fn new() -> Self {
        crate::session::agent_lifecycle::register_builtin_agent_lifecycle_profiles();
        Self::from_profiles(get_agent_profile_contributions())
    }

    fn from_profiles(ordered: Vec<Arc<AgentProfile>>) -> Self {
        let by_name = ordered
            .iter()
            .map(|profile| (profile.name.clone(), Arc::clone(profile)))
            .collect();
        Self { by_name, ordered }
    }
}

impl Default for AgentProfileCatalogService {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentProfileCatalogContract for AgentProfileCatalogService {
    // Original: AgentProfileCatalogService.get().
    fn get(&self, name: &str) -> Option<Arc<AgentProfile>> {
        self.by_name.get(name).cloned()
    }

    // Original: AgentProfileCatalogService.getDefault(). Result is the Rust
    // adaptation of the source's programming-invariant exception.
    fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile> {
        self.get(DEFAULT_AGENT_PROFILE_NAME)
            .ok_or(MissingDefaultAgentProfile)
    }

    // Original: AgentProfileCatalogService.list().
    fn list(&self) -> Vec<Arc<AgentProfile>> {
        self.ordered.clone()
    }
}

pub fn register_agent_profile_catalog_service() {
    register_scoped_service(
        LifecycleScope::App,
        AGENT_PROFILE_CATALOG_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn AgentProfileCatalogContract> =
                Arc::new(AgentProfileCatalogService::new());
            Ok(AgentProfileCatalogHandle(service))
        }),
        InstantiationType::Eager,
        "agentProfileCatalog",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, prompt: &str) -> Arc<AgentProfile> {
        let prompt = prompt.to_owned();
        Arc::new(AgentProfile {
            name: name.into(),
            description: None,
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            system_prompt: Arc::new(move |_| prompt.clone()),
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    #[test]
    fn catalog_preserves_snapshot_order_and_last_duplicate_lookup() {
        let first = profile("agent", "first");
        let other = profile("explore", "other");
        let replacement = profile("agent", "replacement");
        let service = AgentProfileCatalogService::from_profiles(vec![
            first,
            Arc::clone(&other),
            Arc::clone(&replacement),
        ]);

        assert!(Arc::ptr_eq(&service.get_default().unwrap(), &replacement));
        assert_eq!(
            service
                .list()
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            ["agent", "explore", "agent"]
        );
        assert!(Arc::ptr_eq(&service.get("explore").unwrap(), &other));
    }

    #[test]
    fn missing_default_is_an_explicit_invariant_error() {
        let service = AgentProfileCatalogService::from_profiles(Vec::new());
        assert!(matches!(
            service.get_default(),
            Err(MissingDefaultAgentProfile)
        ));
    }
}
