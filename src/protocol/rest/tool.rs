use serde::{Deserialize, Serialize};

use crate::protocol::validation::{literal_true, optional_non_empty};
use crate::protocol::{McpServer, ToolDescriptor};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ListToolsQuery {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "optional_non_empty"
    )]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListToolsResponse {
    pub tools: Vec<ToolDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListMcpServersResponse {
    pub servers: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartMcpServerResult {
    #[serde(deserialize_with = "literal_true")]
    pub restarting: bool,
}
