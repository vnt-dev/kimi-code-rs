//! Model Context Protocol configuration and services.

pub mod client_http;
pub mod client_remote;
pub mod client_shared;
pub mod client_sse;
pub mod client_stdio;
pub mod config_loader;
pub mod config_schema;
pub mod errors;
pub mod mcp_discovery_ops;
pub mod oauth;
pub mod output;
pub mod session_config;
pub mod tool_naming;
pub mod tools;
pub mod types;

pub use client_http::*;
pub use client_remote::*;
pub use client_shared::*;
pub use client_sse::*;
pub use client_stdio::*;
pub use config_loader::*;
pub use config_schema::{
    McpConfigValidationError, McpExecutor, McpServerCommonFields, McpServerConfig,
    McpServerHttpConfig, McpServerRemoteConfig, McpServerSseConfig, McpServerStdioConfig,
    parse_mcp_server_config,
};
pub use errors::*;
pub use mcp_discovery_ops::*;
pub use oauth::*;
pub use output::*;
pub use session_config::*;
pub use tool_naming::*;
pub use types::*;
