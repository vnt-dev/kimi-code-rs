//! OAuth-backed web search.
//!
//! Original: `packages/agent-core-v2/src/app/auth/webSearch`.

pub mod contract;
pub mod providers;
pub mod service;
pub mod tools;

pub use contract::*;
pub use providers::*;
pub use service::*;
pub use tools::*;
