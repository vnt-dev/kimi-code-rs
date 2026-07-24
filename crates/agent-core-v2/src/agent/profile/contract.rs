//! Agent profile data and service contract.
//!
//! Original: `packages/agent-core-v2/src/agent/profile/profile.ts`.

use std::{error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::instantiation::ServiceIdentifier,
        errors::errors::{Error2, Error2Options},
    },
    app::agent_profile_catalog::{AgentProfile, AgentProfileContext},
    kosong::{
        contract::{capability::ModelCapability, provider::ThinkingEffort},
        model::ModelRequestParams,
    },
};

use super::errors::{ProfileErrorCode, ensure_profile_errors_registered};

pub type ProfileError = Error2;
pub type ProfileServiceError = Box<dyn Error + Send + Sync>;

// Original: ProfileError.constructor().
pub fn create_profile_error(
    code: ProfileErrorCode,
    message: impl Into<String>,
    details: Option<Map<String, Value>>,
) -> ProfileError {
    ensure_profile_errors_registered();
    Error2::with_options(
        code.as_str(),
        message,
        Error2Options {
            name: Some("ProfileError".into()),
            details,
            cause: None,
        },
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigData {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    pub model_capabilities: ModelCapability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub thinking_level: String,
    pub system_prompt: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigUpdateData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

pub type SystemPromptContext = AgentProfileContext;
pub type ResolvedAgentProfile = AgentProfile;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileData {
    #[serde(flatten)]
    pub config: AgentConfigData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileUpdateData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_names: Option<Vec<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBindingSnapshot {
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub thinking_level: String,
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disallowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagents: Option<Vec<String>>,
}

pub type ProfileCwdProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;
pub type ProfileChdir =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), ProfileServiceError>> + Send + Sync>;
pub type EmitStatusUpdated = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub enum ProfileCwd {
    Value(String),
    Provider(ProfileCwdProvider),
}

#[derive(Clone, Default)]
pub struct ProfileServiceOptions {
    pub cwd: Option<ProfileCwd>,
    pub chdir: Option<ProfileChdir>,
    pub emit_status_updated: Option<EmitStatusUpdated>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyProfileOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_dirs: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileModelContext {
    pub model_alias: String,
    pub model_capabilities: ModelCapability,
    pub max_output_size: Option<u64>,
    pub always_thinking: Option<bool>,
    pub thinking_level: ThinkingEffort,
    pub reserved_context_size: Option<u64>,
    pub compaction_trigger_ratio: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSetModelResult {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindAgentInput {
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_thinking: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[async_trait]
pub trait AgentProfileServiceContract: Send + Sync {
    fn configure(&self, options: ProfileServiceOptions);
    fn update(&self, changed: ProfileUpdateData) -> Result<(), ProfileServiceError>;
    fn apply_binding_snapshot(
        &self,
        snapshot: ProfileBindingSnapshot,
    ) -> Result<(), ProfileServiceError>;
    async fn bind(&self, input: BindAgentInput) -> Result<(), ProfileServiceError>;
    async fn set_model(&self, model: String) -> Result<ProfileSetModelResult, ProfileServiceError>;
    fn set_thinking(&self, level: String) -> Result<(), ProfileServiceError>;
    fn get_model(&self) -> Result<String, ProfileServiceError>;
    fn use_profile(
        &self,
        profile: ResolvedAgentProfile,
        context: SystemPromptContext,
    ) -> Result<(), ProfileServiceError>;
    async fn apply_profile(
        &self,
        profile: ResolvedAgentProfile,
        options: Option<ApplyProfileOptions>,
    ) -> Result<(), ProfileServiceError>;
    async fn refresh_system_prompt(&self);
    fn get_agents_md_warning(&self) -> Option<String>;
    fn data(&self) -> Result<ProfileData, ProfileServiceError>;
    fn get_effective_thinking_level(&self) -> Result<ThinkingEffort, ProfileServiceError>;
    fn resolve_model_context(&self) -> Result<ProfileModelContext, ProfileServiceError>;
    fn resolve_request_params(&self) -> Result<ModelRequestParams, ProfileServiceError>;
    fn get_model_capabilities(&self) -> Result<ModelCapability, ProfileServiceError>;
    fn get_max_output_size(&self) -> Result<Option<u64>, ProfileServiceError>;
    fn has_model(&self) -> bool;
    fn is_runnable(&self) -> bool;
    fn has_provider(&self) -> bool;
    fn get_system_prompt(&self) -> String;
    fn get_active_tool_names(&self) -> Option<Vec<String>>;
    fn add_active_tool(&self, name: String);
    fn remove_active_tool(&self, name: &str);
}

#[derive(Clone)]
pub struct AgentProfileServiceHandle(pub Arc<dyn AgentProfileServiceContract>);

impl Deref for AgentProfileServiceHandle {
    type Target = dyn AgentProfileServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_PROFILE_SERVICE_ID: ServiceIdentifier<AgentProfileServiceHandle> =
    ServiceIdentifier::new("agentProfileService");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::errors::codes::is_error_code;

    #[test]
    fn profile_error_and_bind_input_preserve_source_identity() {
        let error = create_profile_error(
            ProfileErrorCode::ProfileUnknown,
            "missing",
            Some(Map::from_iter([(
                "profile".into(),
                Value::String("worker".into()),
            )])),
        );
        assert_eq!(error.name, "ProfileError");
        assert_eq!(error.code, "profile.unknown");
        assert_eq!(error.details.as_ref().unwrap()["profile"], "worker");
        assert!(is_error_code("profile.unknown"));
        assert_eq!(AGENT_PROFILE_SERVICE_ID.to_string(), "agentProfileService");
        assert_eq!(
            serde_json::to_value(BindAgentInput {
                profile: "agent".into(),
                model: Some("kimi".into()),
                thinking: None,
                strict_thinking: Some(true),
                cwd: None,
            })
            .unwrap(),
            serde_json::json!({
                "profile": "agent",
                "model": "kimi",
                "strictThinking": true,
            })
        );
    }
}
