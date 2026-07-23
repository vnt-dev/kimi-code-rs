//! Named agent profiles and shared prompt rendering.

pub mod contract;
pub mod contribution;
pub mod profile_shared;
pub mod prompt_prefix;
pub mod service;

pub use contract::{
    AGENT_PROFILE_CATALOG_SERVICE_ID, AgentProfile, AgentProfileCatalogContract,
    AgentProfileCatalogHandle, AgentProfileContext, AgentProfilePromptPrefixContext,
    AgentProfileSummaryPolicy, DEFAULT_AGENT_PROFILE_NAME, MissingDefaultAgentProfile,
};
pub use contribution::{get_agent_profile_contributions, register_agent_profile};
pub use profile_shared::{
    SkillActiveOptions, TASK_AGENT_ROLE_PREFIX, render_prompt_template, render_system_prompt,
    skill_active_for, subagent_allowlist_for, subagent_type_not_allowed_message,
    system_prompt_vars,
};
pub use prompt_prefix::apply_profile_prompt_prefix;
pub use service::{AgentProfileCatalogService, register_agent_profile_catalog_service};
