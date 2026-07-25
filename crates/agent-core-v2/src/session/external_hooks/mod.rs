//! Session-scoped lifecycle and subagent external-hook observation.
//!
//! Original: `packages/agent-core-v2/src/session/externalHooks`.

pub mod contract;
pub mod service;

pub use contract::*;
pub use service::*;
