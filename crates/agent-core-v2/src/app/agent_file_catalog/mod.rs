//! Markdown-defined agent profile discovery.

pub mod agent_file;
pub mod profile_from_file;
pub mod runtime_options;
pub mod types;

pub use agent_file::{AgentFileParseError, ParseAgentFileOptions, parse_agent_file_text};
pub use profile_from_file::agent_profile_from_file;
pub use runtime_options::{
    AGENT_CATALOG_RUNTIME_OPTIONS_ID, AgentCatalogRuntimeOptions,
    agent_catalog_runtime_options_seed, register_agent_catalog_runtime_options,
};
pub use types::{
    AgentFileDefinition, AgentFileDiscoveryResult, AgentFileRoot, AgentFileSource, SkippedAgentFile,
};
