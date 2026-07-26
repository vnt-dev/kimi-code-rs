//! `/init` prompt assets and session initialization support.
//!
//! Original: `packages/agent-core-v2/src/session/sessionInit`.

pub mod contract;
pub mod errors;
pub mod profile;
pub mod service;

pub use contract::*;
pub use errors::*;
pub use profile::*;
pub use service::*;
