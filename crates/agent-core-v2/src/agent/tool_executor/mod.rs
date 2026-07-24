//! Tool execution hooks and Agent-scope executor contract.
//!
//! Original: `packages/agent-core-v2/src/agent/toolExecutor/`.

mod contract;
mod tool_hooks;

pub use contract::*;
pub use tool_hooks::*;
