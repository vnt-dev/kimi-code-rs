//! Process-wide agent profile contributions.
//!
//! Original: `packages/agent-core-v2/src/app/agentProfileCatalog/contribution.ts`.

use std::sync::{Arc, LazyLock, RwLock};

use super::contract::AgentProfile;

static PROFILE_CONTRIBUTIONS: LazyLock<RwLock<Vec<Arc<AgentProfile>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

// Original: registerAgentProfile(). A replacement is removed from its old
// position and appended, preserving the source's contribution order.
pub fn register_agent_profile(definition: AgentProfile) {
    let mut contributions = PROFILE_CONTRIBUTIONS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(index) = contributions
        .iter()
        .position(|profile| profile.name == definition.name)
    {
        contributions.remove(index);
    }
    contributions.push(Arc::new(definition));
}

// Original: getAgentProfileContributions(). Arc cloning snapshots ownership,
// while the catalog service still snapshots the list only at construction.
pub fn get_agent_profile_contributions() -> Vec<Arc<AgentProfile>> {
    PROFILE_CONTRIBUTIONS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::agent_profile_catalog::contract::AgentProfileContext;

    fn profile(name: &str, prompt: &str) -> AgentProfile {
        let prompt = prompt.to_owned();
        AgentProfile {
            name: name.into(),
            description: None,
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            system_prompt: Arc::new(move |_: &AgentProfileContext| prompt.clone()),
            prompt_prefix: None,
            summary_policy: None,
        }
    }

    #[test]
    fn later_registration_replaces_and_moves_a_name_to_the_end() {
        let suffix = uuid::Uuid::new_v4().to_string();
        let replaced_name = format!("test-replaced-{suffix}");
        let middle_name = format!("test-middle-{suffix}");
        register_agent_profile(profile(&replaced_name, "first"));
        register_agent_profile(profile(&middle_name, "middle"));
        register_agent_profile(profile(&replaced_name, "replacement"));

        let matching = get_agent_profile_contributions()
            .into_iter()
            .filter(|profile| profile.name.ends_with(&suffix))
            .collect::<Vec<_>>();
        assert_eq!(
            matching
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            [middle_name, replaced_name]
        );
        assert_eq!(
            matching[1].render_system_prompt(&AgentProfileContext::default()),
            "replacement"
        );
    }
}
