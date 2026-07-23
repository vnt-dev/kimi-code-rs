//! Experimental feature flag definitions and resolution.

pub mod flag_registry;
pub mod flag_registry_service;

pub use flag_registry::{
    FLAG_REGISTRY_SERVICE_ID, FlagDefinitionInput, FlagId, FlagRegistry, FlagRegistryError,
    FlagSurface, get_contributed_flags, register_flag_definition,
};
pub use flag_registry_service::FlagRegistryService;
