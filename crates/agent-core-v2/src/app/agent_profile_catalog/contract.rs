//! Agent profile model and catalog contract.
//!
//! Original: `packages/agent-core-v2/src/app/agentProfileCatalog/agentProfileCatalog.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use futures_util::future::BoxFuture;

use crate::{
    _base::{di::instantiation::ServiceIdentifier, log::Logger},
    session::process::SessionProcessRunnerHandle,
};

pub const DEFAULT_AGENT_PROFILE_NAME: &str = "agent";

#[derive(Clone)]
pub struct AgentProfilePromptPrefixContext {
    pub cwd: String,
    pub runner: SessionProcessRunnerHandle,
    pub log: Option<Arc<dyn Logger>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProfileSummaryPolicy {
    pub min_chars: usize,
    pub continuation_prompt: String,
    pub retries: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentProfileContext {
    pub cwd: Option<String>,
    pub cwd_listing: Option<String>,
    pub agents_md: Option<String>,
    pub additional_dirs_info: Option<String>,
    pub os_kind: Option<String>,
    pub shell_name: Option<String>,
    pub shell_path: Option<String>,
    pub now: Option<String>,
    pub skills: Option<String>,
    pub skill_active: Option<bool>,
    // The source context has an open string index signature. This profile
    // domain key is surfaced separately by `SystemPromptContext` in
    // `agent/profile/profile.ts`.
    pub agents_md_warning: Option<String>,
}

pub type AgentSystemPrompt = Arc<dyn Fn(&AgentProfileContext) -> String + Send + Sync>;
pub type AgentPromptPrefixError = Box<dyn Error + Send + Sync>;
pub type AgentPromptPrefix = Arc<
    dyn Fn(
            AgentProfilePromptPrefixContext,
        ) -> BoxFuture<'static, Result<String, AgentPromptPrefixError>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct AgentProfile {
    pub name: String,
    pub description: Option<String>,
    pub when_to_use: Option<String>,
    pub is_override: Option<bool>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub subagents: Option<Vec<String>>,
    pub model: Option<String>,
    pub system_prompt: AgentSystemPrompt,
    pub prompt_prefix: Option<AgentPromptPrefix>,
    pub summary_policy: Option<AgentProfileSummaryPolicy>,
}

impl AgentProfile {
    pub fn render_system_prompt(&self, context: &AgentProfileContext) -> String {
        (self.system_prompt)(context)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Default agent profile \"{DEFAULT_AGENT_PROFILE_NAME}\" is not registered")]
pub struct MissingDefaultAgentProfile;

pub trait AgentProfileCatalogContract: Send + Sync {
    fn get(&self, name: &str) -> Option<Arc<AgentProfile>>;
    fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile>;
    fn list(&self) -> Vec<Arc<AgentProfile>>;
}

#[derive(Clone)]
pub struct AgentProfileCatalogHandle(pub Arc<dyn AgentProfileCatalogContract>);

impl Deref for AgentProfileCatalogHandle {
    type Target = dyn AgentProfileCatalogContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_PROFILE_CATALOG_SERVICE_ID: ServiceIdentifier<AgentProfileCatalogHandle> =
    ServiceIdentifier::new("agentProfileCatalogService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_renders_with_its_self_contained_system_prompt() {
        let profile = AgentProfile {
            name: "agent".into(),
            description: None,
            when_to_use: None,
            is_override: None,
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: None,
            system_prompt: Arc::new(|context| context.cwd.clone().unwrap_or_default()),
            prompt_prefix: None,
            summary_policy: None,
        };
        assert_eq!(
            profile.render_system_prompt(&AgentProfileContext {
                cwd: Some("/repo".into()),
                ..AgentProfileContext::default()
            }),
            "/repo"
        );
        assert_eq!(
            MissingDefaultAgentProfile.to_string(),
            "Default agent profile \"agent\" is not registered"
        );
        assert_eq!(
            AGENT_PROFILE_CATALOG_SERVICE_ID.to_string(),
            "agentProfileCatalogService"
        );
    }
}
