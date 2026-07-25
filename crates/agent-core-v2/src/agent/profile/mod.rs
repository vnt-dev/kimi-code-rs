//! Agent profile domain.
//! Original: `packages/agent-core-v2/src/agent/profile`.

pub mod context;
pub mod contract;
pub mod errors;
pub mod profile_ops;
pub mod service;

pub use context::*;
pub use contract::*;
pub use errors::*;
pub use profile_ops::*;
pub use service::{AgentProfileService, register_agent_profile_service};
