//! Agent-file profile source contract and discovery projection.
//!
//! Original: `packages/agent-core-v2/src/app/agentFileCatalog/agentProfileSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::event::Event,
    app::agent_profile_catalog::{AgentProfile, AgentSystemPrompt},
};

use super::{AgentFileDiscoveryResult, SkippedAgentFile, agent_profile_from_file};

pub const AGENT_PROFILE_SOURCE_PRIORITY_USER: i32 = 10;
pub const AGENT_PROFILE_SOURCE_PRIORITY_EXTRA: i32 = 20;
pub const AGENT_PROFILE_SOURCE_PRIORITY_PROJECT: i32 = 30;
pub const AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT: i32 = 40;

#[derive(Clone)]
pub struct AgentProfileContribution {
    pub profiles: Vec<Arc<AgentProfile>>,
    pub skipped: Option<Vec<SkippedAgentFile>>,
    pub scanned_roots: Option<Vec<String>>,
}

pub type AgentProfileSourceError = Box<dyn std::error::Error + Send + Sync>;

#[async_trait]
pub trait AgentProfileSourceContract: Send + Sync {
    fn id(&self) -> &str;
    fn priority(&self) -> i32;
    fn on_did_change(&self) -> Option<Event<()>> {
        None
    }
    fn fatal(&self) -> bool {
        false
    }
    async fn load(&self) -> Result<AgentProfileContribution, AgentProfileSourceError>;
}

#[derive(Clone)]
pub struct AgentProfileSourceHandle(pub Arc<dyn AgentProfileSourceContract>);

impl Deref for AgentProfileSourceHandle {
    type Target = dyn AgentProfileSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

// Original: profilesFromDiscovery(). The default prompt closure is captured
// lazily by every profile, so later default-profile/SYSTEM.md changes remain
// visible when the prompt is rendered.
pub fn profiles_from_discovery(
    result: AgentFileDiscoveryResult,
    base_prompt: AgentSystemPrompt,
) -> AgentProfileContribution {
    let profiles = result
        .agents
        .into_iter()
        .map(|definition| {
            Arc::new(agent_profile_from_file(
                definition,
                Arc::clone(&base_prompt),
            ))
        })
        .collect();
    AgentProfileContribution {
        profiles,
        skipped: Some(result.skipped),
        scanned_roots: Some(result.scanned_roots),
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{
        agent_file_catalog::{AgentFileDefinition, AgentFileSource},
        agent_profile_catalog::AgentProfileContext,
    };

    use super::*;

    #[test]
    fn priorities_and_discovery_projection_preserve_source_order_and_laziness() {
        const {
            assert!(AGENT_PROFILE_SOURCE_PRIORITY_USER < AGENT_PROFILE_SOURCE_PRIORITY_EXTRA);
            assert!(AGENT_PROFILE_SOURCE_PRIORITY_EXTRA < AGENT_PROFILE_SOURCE_PRIORITY_PROJECT);
            assert!(AGENT_PROFILE_SOURCE_PRIORITY_PROJECT < AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT);
        }
        let contribution = profiles_from_discovery(
            AgentFileDiscoveryResult {
                agents: vec![AgentFileDefinition {
                    name: "review".into(),
                    description: "Review".into(),
                    when_to_use: None,
                    is_override: false,
                    tools: None,
                    disallowed_tools: None,
                    subagents: None,
                    model: None,
                    prompt: "${base_prompt}".into(),
                    path: "/a.md".into(),
                    source: AgentFileSource::Project,
                }],
                skipped: vec![],
                scanned_roots: vec!["/agents".into()],
            },
            Arc::new(|context: &AgentProfileContext| context.cwd.clone().unwrap_or_default()),
        );
        assert_eq!(contribution.profiles.len(), 1);
        assert_eq!(contribution.scanned_roots, Some(vec!["/agents".into()]));
        assert_eq!(
            contribution.profiles[0].render_system_prompt(&AgentProfileContext {
                cwd: Some("/repo".into()),
                ..AgentProfileContext::default()
            }),
            "/repo"
        );
    }
}
