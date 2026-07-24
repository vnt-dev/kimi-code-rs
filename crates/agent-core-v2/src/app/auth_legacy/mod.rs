//! v1-compatible auth readiness projection.
//! Original: `packages/agent-core-v2/src/app/authLegacy`.
pub mod contract;
pub mod service;
pub use contract::*;
pub use service::{AuthLegacyService, register_auth_legacy_service};
