//! Session-facing MCP configuration resolution.
//!
//! Original: `agent/mcp/session-config.ts`.

use std::{collections::HashMap, path::PathBuf};

use super::{LoadMcpServersInput, McpConfigLoadError, McpServerConfig, load_mcp_servers};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionMcpConfig {
    pub servers: HashMap<String, McpServerConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveSessionMcpConfigInput {
    pub cwd: PathBuf,
    pub home_dir: Option<PathBuf>,
}

// Original: resolveSessionMcpConfig().
pub async fn resolve_session_mcp_config(
    input: &ResolveSessionMcpConfigInput,
) -> Result<Option<SessionMcpConfig>, McpConfigLoadError> {
    let servers = load_mcp_servers(&LoadMcpServersInput {
        cwd: input.cwd.clone(),
        home_dir: input.home_dir.clone(),
    })
    .await?;
    Ok((!servers.is_empty()).then_some(SessionMcpConfig { servers }))
}

// Original: mergeCallerMcpServers(). Caller-provided names replace the loaded
// file configuration while an absent/empty caller object preserves `base`.
pub fn merge_caller_mcp_servers(
    base: Option<&SessionMcpConfig>,
    caller_servers: Option<&HashMap<String, McpServerConfig>>,
) -> Option<SessionMcpConfig> {
    let Some(caller_servers) = caller_servers.filter(|servers| !servers.is_empty()) else {
        return base.cloned();
    };
    let mut servers = base.map_or_else(HashMap::new, |base| base.servers.clone());
    servers.extend(caller_servers.clone());
    Some(SessionMcpConfig { servers })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::mcp::{McpServerCommonFields, McpServerConfig, McpServerStdioConfig};

    fn stdio(command: &str) -> McpServerConfig {
        McpServerConfig::Stdio(McpServerStdioConfig {
            command: command.into(),
            args: None,
            env: None,
            cwd: None,
            executor: None,
            common: McpServerCommonFields::default(),
        })
    }

    #[test]
    fn caller_servers_overlay_file_servers_and_empty_callers_preserve_base() {
        let base = SessionMcpConfig {
            servers: HashMap::from([
                ("shared".into(), stdio("file")),
                ("file".into(), stdio("file")),
            ]),
        };
        assert_eq!(
            merge_caller_mcp_servers(Some(&base), None),
            Some(base.clone())
        );
        assert_eq!(
            merge_caller_mcp_servers(Some(&base), Some(&HashMap::new())),
            Some(base.clone())
        );
        let merged = merge_caller_mcp_servers(
            Some(&base),
            Some(&HashMap::from([
                ("shared".into(), stdio("caller")),
                ("caller".into(), stdio("caller")),
            ])),
        )
        .unwrap();
        assert_eq!(merged.servers.len(), 3);
        assert_eq!(
            merged.servers["shared"],
            McpServerConfig::Stdio(McpServerStdioConfig {
                command: "caller".into(),
                args: None,
                env: None,
                cwd: None,
                executor: None,
                common: McpServerCommonFields::default(),
            })
        );
    }
}
