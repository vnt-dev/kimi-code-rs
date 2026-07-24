//! Shared remote-MCP configuration helpers.
//!
//! Original: `agent/mcp/client-remote.ts`.

use std::collections::HashMap;

use super::{McpServerConfig, McpServerRemoteConfig};

#[derive(Clone, Debug, thiserror::Error, Eq, PartialEq)]
#[error("MCP {transport} bearer token env var \"{variable}\" is not set or is empty")]
pub struct McpRemoteHeaderError {
    pub transport: String,
    pub variable: String,
}

// Original: buildMcpRemoteHeaders().
pub fn build_mcp_remote_headers(
    config: &McpServerRemoteConfig,
    transport: &str,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Result<Option<HashMap<String, String>>, McpRemoteHeaderError> {
    let mut headers = config.headers.clone().unwrap_or_default();
    if let Some(variable) = &config.bearer_token_env_var {
        let token = env_lookup(variable)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| McpRemoteHeaderError {
                transport: transport.to_ascii_uppercase(),
                variable: variable.clone(),
            })?;
        headers.retain(|name, _| !name.eq_ignore_ascii_case("authorization"));
        headers.insert("Authorization".into(), format!("Bearer {token}"));
    }
    Ok((!headers.is_empty()).then_some(headers))
}

// Original: isRemoteMcpConfig().
pub const fn is_remote_mcp_config(config: &McpServerConfig) -> bool {
    matches!(config, McpServerConfig::Http(_) | McpServerConfig::Sse(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::mcp::McpServerCommonFields;

    fn remote() -> McpServerRemoteConfig {
        McpServerRemoteConfig {
            url: "https://example.com/mcp".into(),
            headers: Some(HashMap::from([
                ("Authorization".into(), "old".into()),
                ("X-Test".into(), "yes".into()),
            ])),
            bearer_token_env_var: Some("TOKEN".into()),
            common: McpServerCommonFields::default(),
        }
    }

    #[test]
    fn bearer_tokens_replace_case_insensitive_authorization_headers() {
        let headers = build_mcp_remote_headers(&remote(), "http", |name| {
            (name == "TOKEN").then(|| "secret".into())
        })
        .unwrap()
        .unwrap();
        assert_eq!(headers["Authorization"], "Bearer secret");
        assert_eq!(headers["X-Test"], "yes");
        assert_eq!(
            build_mcp_remote_headers(&remote(), "sse", |_| None)
                .unwrap_err()
                .to_string(),
            "MCP SSE bearer token env var \"TOKEN\" is not set or is empty"
        );
    }
}
