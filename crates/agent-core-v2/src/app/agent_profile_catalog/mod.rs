//! Named agent profiles and shared prompt rendering.

pub mod profile_shared;

pub use profile_shared::{
    AgentProfileContext, SkillActiveOptions, TASK_AGENT_ROLE_PREFIX, render_prompt_template,
    render_system_prompt, skill_active_for, subagent_allowlist_for,
    subagent_type_not_allowed_message, system_prompt_vars,
};
