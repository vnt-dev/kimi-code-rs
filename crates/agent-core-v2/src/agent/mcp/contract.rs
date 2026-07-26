//! Agent-facing MCP service contract.
//!
//! Original: `agent/mcp/mcp.ts`.

use std::{collections::HashSet, ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{Disposable, DisposeResult},
        },
        event::Event,
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{AbortError, AbortSignal},
    },
    kosong::contract::tool::Tool,
};

use super::{McpClient, McpOAuthService, McpServerEntry, McpToolDefinition};

#[derive(Clone)]
pub struct McpResolvedServer {
    pub client: Arc<dyn McpClient>,
    pub tools: Vec<Tool>,
    pub raw_tools: Vec<McpToolDefinition>,
    pub enabled_names: HashSet<String>,
}

#[async_trait]
pub trait AgentMcpServiceContract: Disposable + Send + Sync {
    fn oauth_service(&self) -> Option<Arc<McpOAuthService>>;
    async fn wait_for_initial_load(
        &self,
        signal: Option<&AbortSignal>,
    ) -> Result<(), Arc<AbortError>>;
    async fn initial_load_duration_ms(&self) -> u128;
    async fn list(&self) -> Vec<McpServerEntry>;
    async fn resolved(&self, name: &str) -> Option<McpResolvedServer>;
    async fn get_remote_server_url(&self, name: &str) -> Option<String>;
    async fn reconnect(&self, name: &str, signal: Option<&AbortSignal>) -> Result<(), BoxError>;
    fn on_status_change(&self) -> Event<McpServerEntry>;
}

#[derive(Clone)]
pub struct AgentMcpServiceHandle(pub Arc<dyn AgentMcpServiceContract>);

impl Deref for AgentMcpServiceHandle {
    type Target = dyn AgentMcpServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentMcpServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_MCP_SERVICE_ID: ServiceIdentifier<AgentMcpServiceHandle> =
    ServiceIdentifier::new("agentMcpService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_identifier_matches_source_decorator() {
        assert_eq!(AGENT_MCP_SERVICE_ID.to_string(), "agentMcpService");
    }
}
