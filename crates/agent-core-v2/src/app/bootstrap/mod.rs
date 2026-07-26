//! Frozen process-startup facts and application path layout.

mod composition;
pub mod options;
pub mod service;

pub use composition::{BootstrapResult, bootstrap, bootstrap_seed, bootstrap_with_extra};
pub use options::{
    BOOTSTRAP_OPTIONS_ID, BootstrapInput, BootstrapOptions, BootstrapResolveError,
    ResolveConfigPathInput, ensure_kimi_home, resolve_bootstrap_options, resolve_config_path,
    resolve_config_path_with_environment, resolve_kimi_home, resolve_kimi_home_with_environment,
};
pub use service::{
    BOOTSTRAP_SERVICE_ID, BootstrapService, BootstrapServiceContract, BootstrapServiceHandle,
    PersistenceScopeName, register_bootstrap_service,
};
