//! Pure session agent-profile catalog merge logic.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/sessionAgentProfileCatalogService.ts`,
//! `SessionAgentProfileCatalogService.remerge()`.

use std::{cmp::Reverse, sync::Arc};

use indexmap::IndexMap;

use crate::app::{
    agent_file_catalog::AgentProfileContribution, agent_profile_catalog::AgentProfile,
};

pub struct ProfileContributionWithPriority {
    pub contribution: AgentProfileContribution,
    pub priority: i32,
}

// Original: remerge(). Contributions are processed highest priority first;
// within one contribution the last same-named profile wins, matching JS Map#set.
pub fn merge_agent_profiles(
    builtin: impl IntoIterator<Item = Arc<AgentProfile>>,
    contributions: impl IntoIterator<Item = ProfileContributionWithPriority>,
    mut warn: impl FnMut(&str),
) -> IndexMap<String, Arc<AgentProfile>> {
    let mut merged = builtin
        .into_iter()
        .map(|profile| (profile.name.clone(), profile))
        .collect::<IndexMap<_, _>>();
    let mut contributions = contributions.into_iter().collect::<Vec<_>>();
    contributions.sort_by_key(|contribution| Reverse(contribution.priority));

    let mut candidates_by_name = IndexMap::<String, Vec<Arc<AgentProfile>>>::new();
    for contribution in contributions {
        let mut source_profiles = IndexMap::<String, Arc<AgentProfile>>::new();
        for profile in contribution.contribution.profiles {
            source_profiles.insert(profile.name.clone(), profile);
        }
        for profile in source_profiles.into_values() {
            candidates_by_name
                .entry(profile.name.clone())
                .or_default()
                .push(profile);
        }
    }
    for candidates in candidates_by_name.into_values() {
        for profile in candidates {
            if merged.contains_key(&profile.name) && profile.is_override != Some(true) {
                warn(&format!(
                    "agent file profile \"{}\" ignored: a same-name builtin profile exists; set \"override: true\" in the frontmatter to replace it",
                    profile.name
                ));
                continue;
            }
            merged.insert(profile.name.clone(), profile);
            break;
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::app::agent_profile_catalog::AgentSystemPrompt;

    use super::*;

    fn profile(name: &str, prompt: &str, is_override: bool) -> Arc<AgentProfile> {
        let prompt: AgentSystemPrompt = Arc::new({
            let prompt = prompt.to_owned();
            move |_| prompt.clone()
        });
        Arc::new(AgentProfile {
            name: name.into(),
            description: None,
            when_to_use: None,
            is_override: Some(is_override),
            tools: None,
            disallowed_tools: None,
            subagents: None,
            system_prompt: prompt,
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    fn contribution(
        profiles: Vec<Arc<AgentProfile>>,
        priority: i32,
    ) -> ProfileContributionWithPriority {
        ProfileContributionWithPriority {
            contribution: AgentProfileContribution {
                profiles,
                skipped: None,
                scanned_roots: None,
            },
            priority,
        }
    }

    #[test]
    fn merge_requires_override_for_builtins_and_preserves_priority_and_tail_wins() {
        let warnings = Mutex::new(Vec::new());
        let result = merge_agent_profiles(
            vec![
                profile("agent", "builtin", false),
                profile("builtin-only", "builtin", false),
            ],
            vec![
                contribution(vec![profile("agent", "user-no", false)], 10),
                contribution(vec![profile("agent", "project", true)], 30),
                contribution(
                    vec![
                        profile("review", "early", false),
                        profile("review", "tail", false),
                    ],
                    20,
                ),
            ],
            |message| warnings.lock().unwrap().push(message.to_owned()),
        );
        assert_eq!(
            result["agent"].render_system_prompt(&Default::default()),
            "project"
        );
        assert_eq!(
            result["review"].render_system_prompt(&Default::default()),
            "tail"
        );
        assert!(result.contains_key("builtin-only"));
        assert!(warnings.lock().unwrap().is_empty());

        let fallback = merge_agent_profiles(
            vec![profile("agent", "builtin", false)],
            vec![
                contribution(vec![profile("agent", "ignored", false)], 40),
                contribution(vec![profile("agent", "fallback", true)], 10),
            ],
            |message| warnings.lock().unwrap().push(message.to_owned()),
        );
        assert_eq!(
            fallback["agent"].render_system_prompt(&Default::default()),
            "fallback"
        );
        assert_eq!(warnings.lock().unwrap().len(), 1);
    }
}
