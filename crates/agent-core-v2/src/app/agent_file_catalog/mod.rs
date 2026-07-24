//! Markdown-defined agent profile discovery.

pub mod agent_file;
pub mod agent_profile_source;
pub mod agent_roots;
pub mod file_discovery;
pub mod paths;
pub mod profile_from_file;
pub mod runtime_options;
pub mod system_file;
pub mod types;

pub use agent_file::{AgentFileParseError, ParseAgentFileOptions, parse_agent_file_text};
pub use agent_profile_source::{
    AGENT_PROFILE_SOURCE_PRIORITY_EXPLICIT, AGENT_PROFILE_SOURCE_PRIORITY_EXTRA,
    AGENT_PROFILE_SOURCE_PRIORITY_PROJECT, AGENT_PROFILE_SOURCE_PRIORITY_USER,
    AgentProfileContribution, AgentProfileSourceContract, AgentProfileSourceError,
    AgentProfileSourceHandle, profiles_from_discovery,
};
pub use agent_roots::{
    AgentRootWarn, configured_agent_roots, project_agent_roots, user_agent_roots,
};
pub use file_discovery::{DiscoverAgentFilesWarn, discover_agent_files};
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
