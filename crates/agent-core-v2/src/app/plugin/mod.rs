//! Plugin installation, manifests, and runtime contributions.

pub mod archive;
pub mod commands;
pub mod contract;
pub mod errors;
pub mod github_resolver;
pub mod manager;
pub mod manifest;
pub mod plugin_service;
pub mod source;
pub mod store;
pub mod types;

pub use archive::{PluginArchiveError, download_zip, extract_zip};
pub use commands::{
    LoadPluginCommandOptions, ParseCommandError, ParseCommandTextOptions, expand_command_arguments,
    load_plugin_command, parse_command_text,
};
pub use contract::{
    GetPluginInfoInput, InstallPluginInput, PLUGIN_SERVICE_ID, PluginServiceContract,
    PluginServiceError, PluginServiceHandle, PluginServiceResult, RemovePluginInput,
    SetPluginEnabledInput, SetPluginMcpServerEnabledInput,
};
pub use errors::{
    PLUGIN_ERRORS, PLUGIN_LOAD_FAILED, PLUGIN_NOT_FOUND, ensure_plugin_errors_registered,
};
pub use github_resolver::{
    GithubResolverError, GithubSourceInput, GithubSourceResolution, resolve_github_commit_sha,
    resolve_github_source,
};
pub use manager::{PluginManager, PluginManagerError, PluginManagerOptions, PluginManagerResult};
pub use manifest::{ParsedManifestResult, parse_manifest};
pub use plugin_service::{PluginService, register_plugin_service};
pub use source::{
    InstallSource, ResolveInstallSourceError, ResolvedSource, resolve_install_source,
};
pub use store::{
    InstalledFile, InstalledRecord, InstalledStoreError, read_installed, write_installed,
};
pub use types::*;
