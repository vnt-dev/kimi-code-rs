//! Experimental feature flag definitions and resolution.

pub mod contract;
pub mod flag_registry;
pub mod flag_registry_service;

pub use contract::*;
pub use flag_registry::{
    FLAG_REGISTRY_SERVICE_ID, FlagDefinitionInput, FlagId, FlagRegistry, FlagRegistryError,
    FlagRegistryHandle, FlagSurface, get_contributed_flags, register_flag_definition,
};
pub use flag_registry_service::{FlagRegistryService, register_flag_registry_service};
