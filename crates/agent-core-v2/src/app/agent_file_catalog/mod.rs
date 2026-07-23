//! Markdown-defined agent profile discovery.

pub mod agent_file;
pub mod profile_from_file;
pub mod types;

pub use agent_file::{AgentFileParseError, ParseAgentFileOptions, parse_agent_file_text};
pub use profile_from_file::agent_profile_from_file;
pub use types::{
    AgentFileDefinition, AgentFileDiscoveryResult, AgentFileRoot, AgentFileSource, SkippedAgentFile,
};
