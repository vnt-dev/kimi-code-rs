//! Built-in product capabilities (kimi-cu, kimi-webbridge): layered readiness
//! detection and idempotent install orchestration.
//!
//! Original: `packages/agent-core-v2/src/app/capability/`.

pub mod capability_service;
pub mod contract;
pub mod entries;
pub mod errors;
pub mod host;
pub mod types;

pub use capability_service::{CapabilityService, register_capability_service};
pub use contract::{
    CAPABILITY_SERVICE_ID, CapabilityServiceContract, CapabilityServiceError,
    CapabilityServiceHandle, CapabilityServiceResult,
};
pub use errors::{
    CAPABILITY_ERRORS, CAPABILITY_INSTALL_IN_PROGRESS, CAPABILITY_NOT_FOUND,
    CAPABILITY_UNSUPPORTED, ensure_capability_errors_registered,
};
pub use host::{
    CommandResult, DOWNLOAD_IDLE_TIMEOUT, FetchBodyStream, FetchLike, FetchResponse, ReqwestFetch,
    download_to_file, rm_force, run_command,
};
pub use types::*;
