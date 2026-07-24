//! v1-compatible session message-history contract.
//! Original: `packages/agent-core-v2/src/app/messageLegacy`.
pub mod contract;
pub mod errors;
pub use contract::*;
pub use errors::{MESSAGE_NOT_FOUND, ensure_message_legacy_errors_registered};
