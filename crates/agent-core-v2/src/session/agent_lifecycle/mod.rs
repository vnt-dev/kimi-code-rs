//! Session agent lifecycle support.
//!
//! Original: `session/agentLifecycle`.

pub mod contract;
pub mod errors;
pub mod main_agent;
pub mod profiles;
pub mod registry;
pub mod service;
pub mod subagent_metadata;

pub use contract::*;
pub use errors::*;
pub use main_agent::*;
pub use profiles::register_builtin_agent_lifecycle_profiles;
pub use registry::*;
pub use service::*;
pub use subagent_metadata::*;
