//! Frozen process-startup facts and application path layout.

pub mod options;
pub mod service;

pub use options::{
    BOOTSTRAP_OPTIONS_ID, BootstrapInput, BootstrapOptions, BootstrapResolveError,
    ensure_kimi_home, resolve_bootstrap_options, resolve_config_path, resolve_kimi_home,
};
pub use service::{
    BOOTSTRAP_SERVICE_ID, BootstrapService, BootstrapServiceContract, BootstrapServiceHandle,
    PersistenceScopeName, register_bootstrap_service,
};
