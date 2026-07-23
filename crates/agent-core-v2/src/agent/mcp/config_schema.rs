//! MCP server configuration models and schema validation.
//!
//! Original: `packages/agent-core-v2/src/agent/mcp/config-schema.ts`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerCommonFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_tools: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpExecutor {
    Local,
    Kaos,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStdioConfig {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<McpExecutor>,
    #[serde(flatten)]
    pub common: McpServerCommonFields,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRemoteConfig {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token_env_var: Option<String>,
    #[serde(flatten)]
    pub common: McpServerCommonFields,
}

pub type McpServerHttpConfig = McpServerRemoteConfig;
pub type McpServerSseConfig = McpServerRemoteConfig;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum McpServerConfig {
    Stdio(McpServerStdioConfig),
    Http(McpServerHttpConfig),
    Sse(McpServerSseConfig),
}

impl McpServerConfig {
    pub const fn transport(&self) -> &'static str {
        match self {
            Self::Stdio(_) => "stdio",
            Self::Http(_) => "http",
            Self::Sse(_) => "sse",
        }
    }

    pub fn common(&self) -> &McpServerCommonFields {
        match self {
            Self::Stdio(config) => &config.common,
            Self::Http(config) | Self::Sse(config) => &config.common,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid MCP server configuration: {message}")]
pub struct McpConfigValidationError {
    pub message: String,
}

impl McpConfigValidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

// Original: McpServerConfigSchema preprocess + discriminated-union parse.
pub fn parse_mcp_server_config(raw: &Value) -> Result<McpServerConfig, McpConfigValidationError> {
    let mut normalized = raw.clone();
    if let Some(object) = normalized.as_object_mut()
        && !object.contains_key("transport")
    {
        if object.get("command").is_some_and(Value::is_string) {
            object.insert("transport".into(), Value::String("stdio".into()));
        } else if object.get("url").is_some_and(Value::is_string) {
            object.insert("transport".into(), Value::String("http".into()));
        }
    }
    let config = serde_json::from_value::<McpServerConfig>(normalized)
        .map_err(|error| McpConfigValidationError::new(error.to_string()))?;
    validate(&config)?;
    Ok(config)
}

fn validate(config: &McpServerConfig) -> Result<(), McpConfigValidationError> {
    validate_common(config.common())?;
    match config {
        McpServerConfig::Stdio(config) => {
            if config.command.is_empty() {
                return Err(McpConfigValidationError::new(
                    "stdio command must contain at least one character",
                ));
            }
        }
        McpServerConfig::Http(config) | McpServerConfig::Sse(config) => {
            url::Url::parse(&config.url)
                .map_err(|_| McpConfigValidationError::new("remote URL must be a valid URL"))?;
            if config
                .bearer_token_env_var
                .as_ref()
                .is_some_and(String::is_empty)
            {
                return Err(McpConfigValidationError::new(
                    "bearerTokenEnvVar must contain at least one character",
                ));
            }
        }
    }
    Ok(())
}

fn validate_common(common: &McpServerCommonFields) -> Result<(), McpConfigValidationError> {
    if common.startup_timeout_ms == Some(0) {
        return Err(McpConfigValidationError::new(
            "startupTimeoutMs must be a positive integer",
        ));
    }
    if common.tool_timeout_ms == Some(0) {
        return Err(McpConfigValidationError::new(
            "toolTimeoutMs must be a positive integer",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn infers_legacy_transports_and_preserves_camel_case_wire_fields() {
        let stdio = parse_mcp_server_config(&json!({
            "command": "node",
            "args": ["server.js"],
            "startupTimeoutMs": 5,
            "unknown": "stripped"
        }))
        .unwrap();
        assert_eq!(stdio.transport(), "stdio");
        let serialized = serde_json::to_value(&stdio).unwrap();
        assert_eq!(serialized["transport"], "stdio");
        assert_eq!(serialized["startupTimeoutMs"], 5);
        assert!(serialized.get("unknown").is_none());

        let http = parse_mcp_server_config(&json!({"url": "https://example.com/mcp"})).unwrap();
        assert_eq!(http.transport(), "http");
    }

    #[test]
    fn explicit_transport_wins_and_invalid_constraints_are_rejected() {
        assert_eq!(
            parse_mcp_server_config(&json!({
                "transport": "sse",
                "url": "https://example.com/events",
                "command": "ignored"
            }))
            .unwrap()
            .transport(),
            "sse"
        );
        for invalid in [
            json!({"command": ""}),
            json!({"command": "node", "toolTimeoutMs": 0}),
            json!({"url": "not a url"}),
            json!({"transport": "http", "url": "https://example.com", "bearerTokenEnvVar": ""}),
            json!({"enabled": true}),
        ] {
            assert!(parse_mcp_server_config(&invalid).is_err(), "{invalid}");
        }
    }
}
