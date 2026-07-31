//! Agent-scoped skill activation contract.
//!
//! Original: `packages/agent-core-v2/src/agent/skill/skill.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    _base::di::{
        instantiation::ServiceIdentifier,
        lifecycle::{Disposable, DisposeResult},
    },
    agent::{context_memory::SkillActivationOrigin, loop_::TurnHandle},
};

pub type AgentSkillServiceError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillActivationInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSkillPrompt {
    pub content: String,
    pub origins: Vec<SkillActivationOrigin>,
}

#[async_trait]
pub trait AgentSkillServiceContract: Disposable + Send + Sync {
    async fn activate(
        &self,
        input: SkillActivationInput,
    ) -> Result<TurnHandle, AgentSkillServiceError>;
    async fn prepare_prompt_skills(
        &self,
        inputs: Vec<SkillActivationInput>,
        shared_args: Option<String>,
    ) -> Result<PreparedSkillPrompt, AgentSkillServiceError>;
    fn record_user_activations(&self, origins: &[SkillActivationOrigin]);
    fn record_model_tool_activation(&self, origin: SkillActivationOrigin);
}

#[derive(Clone)]
pub struct AgentSkillServiceHandle(pub Arc<dyn AgentSkillServiceContract>);

impl Deref for AgentSkillServiceHandle {
    type Target = dyn AgentSkillServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentSkillServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_SKILL_SERVICE_ID: ServiceIdentifier<AgentSkillServiceHandle> =
    ServiceIdentifier::new("agentSkillService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_activation_input_match_source() {
        assert_eq!(AGENT_SKILL_SERVICE_ID.to_string(), "agentSkillService");
        assert_eq!(
            serde_json::to_value(SkillActivationInput {
                name: "review".into(),
                args: Some("--strict".into())
            })
            .unwrap(),
            serde_json::json!({"name": "review", "args": "--strict"})
        );
    }
}
