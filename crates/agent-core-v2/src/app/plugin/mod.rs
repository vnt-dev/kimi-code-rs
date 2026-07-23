//! Plugin installation, manifests, and runtime contributions.

pub mod commands;
pub mod errors;
pub mod source;
pub mod types;

pub use commands::{
    LoadPluginCommandOptions, ParseCommandError, ParseCommandTextOptions, expand_command_arguments,
    load_plugin_command, parse_command_text,
};
pub use errors::{
    PLUGIN_ERRORS, PLUGIN_LOAD_FAILED, PLUGIN_NOT_FOUND, ensure_plugin_errors_registered,
};
pub use source::{
    InstallSource, ResolveInstallSourceError, ResolvedSource, resolve_install_source,
};
pub use types::*;
