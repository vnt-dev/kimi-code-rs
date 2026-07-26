//! External process hook configuration and execution.

pub mod config_section;
pub mod contract;
pub mod runner;
pub mod service;
pub mod types;
pub mod user_prompt;

pub use config_section::{
    HOOKS_CONFIG_SCHEMA, HOOKS_SECTION, HookDefConfig, HooksConfig, hooks_from_toml, hooks_to_toml,
    parse_hook_def_config, register_hooks_config_section,
};
pub use contract::*;
pub use runner::{RunHookOptions, build_hook_spawn_options, run_hook};
pub use service::{
    AgentExternalHooksService, HookResultEvent, register_agent_external_hooks_service,
};
pub use types::{
    HOOK_EVENT_TYPES, HookAction, HookBlockDecision, HookDef, HookEventType, HookMatcherValue,
    HookResult,
};
pub use user_prompt::{
    RenderedHookResult, render_hook_result, render_user_prompt_hook_block_result,
    render_user_prompt_hook_result,
};
