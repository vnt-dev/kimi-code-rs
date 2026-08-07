//! Markdown-defined agent profile discovery.

pub mod agent_file;
pub mod agent_profile_source;
pub mod agent_roots;
pub mod config_section;
pub mod file_discovery;
pub mod managed_files;
pub mod paths;
pub mod profile_from_file;
pub mod runtime_options;
pub mod system_file;
pub mod types;
pub mod user_file_agent_source;

pub use agent_file::{AgentFileParseError, ParseAgentFileOptions, parse_agent_file_text};
pub use agent_profile_source::{
    AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT, AGENT_PROFILE_SOURCE_PRIORITY_EXTRA,
    AGENT_PROFILE_SOURCE_PRIORITY_PROJECT, AGENT_PROFILE_SOURCE_PRIORITY_USER,
    AgentProfileContribution, AgentProfileSourceContract, AgentProfileSourceError,
    AgentProfileSourceHandle, profiles_from_discovery,
};
pub use agent_roots::{
    AgentRootWarn, configured_agent_roots, project_agent_roots, resolve_agent_project_root,
    user_agent_roots,
};
pub use config_section::{
    EXTRA_AGENT_DIRS_CONFIG_SCHEMA, EXTRA_AGENT_DIRS_SECTION,
    register_agent_file_catalog_config_sections,
};
pub use file_discovery::{DiscoverAgentFilesWarn, discover_agent_files};
pub use managed_files::{
    ManagedAgentFile, delete_managed_agent_file, list_managed_agent_files, save_managed_agent_file,
};
pub use paths::{is_directory_path, is_file_path, path_exists, resolve_agent_path};
pub use profile_from_file::agent_profile_from_file;
pub use runtime_options::{
    AGENT_CATALOG_RUNTIME_OPTIONS_ID, AgentCatalogRuntimeOptions,
    agent_catalog_runtime_options_seed, register_agent_catalog_runtime_options,
};
pub use system_file::{SYSTEM_MD_FILENAME, load_system_md_profile};
pub use types::{
    AgentFileDefinition, AgentFileDiscoveryResult, AgentFileRoot, AgentFileSource, SkippedAgentFile,
};
pub use user_file_agent_source::{
    USER_FILE_AGENT_SOURCE_ID, UserFileAgentSource, UserFileAgentSourceHandle,
    register_user_file_agent_source,
};
