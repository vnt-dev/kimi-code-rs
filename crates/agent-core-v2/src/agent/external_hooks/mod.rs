//! External process hook configuration and execution.

pub mod config_section;
pub mod types;

pub use config_section::{
    HOOKS_CONFIG_SCHEMA, HOOKS_SECTION, HookDefConfig, HooksConfig, hooks_from_toml, hooks_to_toml,
    register_hooks_config_section,
};
pub use types::{
    HOOK_EVENT_TYPES, HookAction, HookBlockDecision, HookDef, HookEventType, HookMatcherValue,
    HookResult,
};
