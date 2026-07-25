//! Kimi Code application protocol server.
//!
//! This crate is the Rust counterpart of
//! `packages/kap-server/src/index.ts`. Protocol DTOs live in the sibling
//! `kimi-code-protocol` crate and are re-exported here where the TypeScript
//! package exposed them directly.

pub mod instance_registry;
pub mod launch;
pub mod middleware;
pub mod request_id;
pub mod request_logging;
pub mod routes;
pub mod security;
pub mod services;
pub mod start;
pub mod transport;
pub mod version;
pub mod web;

pub use instance_registry::{
    HEARTBEAT_INTERVAL, InstanceRegistration, InstanceRegistry, InstanceRegistryOptions,
    ServerInstanceInfo, get_live_server_instance, list_live_server_instances,
    resolve_server_instances_dir,
};
pub use kimi_code_protocol::{Envelope, err_envelope, ok_envelope};
pub use security::bind_classify::{BindClass, ClassifyOptions, classify};
pub use services::auth::persistent_token::{rotate_server_token, server_token_path};
pub use start::{RunningServer, ServerStartOptions, StartServerError, start_server};
