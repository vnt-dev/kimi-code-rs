//! Model Context Protocol configuration and services.

pub mod config_schema;

pub use config_schema::{
    McpConfigValidationError, McpExecutor, McpServerCommonFields, McpServerConfig,
    McpServerHttpConfig, McpServerRemoteConfig, McpServerSseConfig, McpServerStdioConfig,
    parse_mcp_server_config,
};
