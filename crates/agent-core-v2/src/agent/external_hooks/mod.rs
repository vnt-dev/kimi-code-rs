//! External process hook configuration and execution.

pub mod types;

pub use types::{
    HOOK_EVENT_TYPES, HookAction, HookBlockDecision, HookDef, HookEventType, HookMatcherValue,
    HookResult,
};
