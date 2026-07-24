//! Model Context Protocol configuration and services.

pub mod config_schema;
pub mod output;
pub mod tool_naming;
pub mod tools;
pub mod types;

pub use config_schema::{
    McpConfigValidationError, McpExecutor, McpServerCommonFields, McpServerConfig,
    McpServerHttpConfig, McpServerRemoteConfig, McpServerSseConfig, McpServerStdioConfig,
    parse_mcp_server_config,
};
pub use output::*;
pub use tool_naming::*;
pub use types::*;
