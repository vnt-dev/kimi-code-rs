//! Built-in agent permission policies.
//!
//! Original: `agent/permissionPolicy/policies`.

pub mod default_tool_approve;
pub mod deny_all;
pub mod fallback_ask;

pub use default_tool_approve::*;
pub use deny_all::*;
pub use fallback_ask::*;
