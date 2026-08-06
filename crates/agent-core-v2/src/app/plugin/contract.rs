//! App-scoped plugin management and contribution contract.
//!
//! Original: `packages/agent-core-v2/src/app/plugin/plugin.ts`.
//!
//! Rust adaptation: named `contract` to avoid a `plugin::plugin` module.

use std::{collections::HashMap, error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{Disposable, DisposeResult},
        },
        event::Event,
    },
    agent::{external_hooks::HookDef, mcp::McpServerConfig},
    app::skill_catalog::SkillRoot,
};

use super::types::{
    EnabledPluginSessionStart, PluginCommandDef, PluginInfo, PluginInstallOperation,
    PluginInstallProgressCallback, PluginSummary, PluginUpdateStatus, ReloadSummary,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallPluginInput {
    pub source: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPluginEnabledInput {
    pub id: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPluginMcpServerEnabledInput {
    pub id: String,
    pub server: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemovePluginInput {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetPluginInfoInput {
    pub id: String,
}

pub type PluginServiceError = Box<dyn Error + Send + Sync>;
pub type PluginServiceResult<T> = Result<T, PluginServiceError>;

#[async_trait]
pub trait PluginServiceContract: Disposable + Send + Sync {
    async fn list_plugins(&self) -> PluginServiceResult<Vec<PluginSummary>>;
    async fn install_plugin(&self, input: InstallPluginInput)
    -> PluginServiceResult<PluginSummary>;
    async fn install_plugin_with_progress(
        &self,
        input: InstallPluginInput,
        _progress: PluginInstallProgressCallback,
    ) -> PluginServiceResult<PluginSummary> {
        self.install_plugin(input).await
    }
    /// Starts an install on a background task and returns immediately;
    /// progress and the terminal result live in memory and are polled through
    /// `plugin_install_progress`. Mirrors the capability install model so
    /// transports without an event channel (web RPC) can follow installs.
    async fn install_plugin_in_background(
        self: Arc<Self>,
        input: InstallPluginInput,
        operation_id: String,
    ) -> PluginServiceResult<()>;
    /// Returns the current snapshot of a background install. A finished
    /// operation (complete or failed) is removed by the read — it is reported
    /// exactly once.
    fn plugin_install_progress(&self, operation_id: &str) -> Option<PluginInstallOperation>;
    /// Returns the operation that a newly-mounted client should resume.
    /// The service currently permits only one background install at a time.
    fn list_plugin_install_operations(&self) -> Vec<PluginInstallOperation> {
        Vec::new()
    }
    /// Prevents a capability's wiring plugin from being removed while the
    /// capability is still installing runtime layers around it.
    async fn reserve_plugin_removal(&self, _id: &str) -> PluginServiceResult<()> {
        Ok(())
    }
    fn release_plugin_removal(&self, _id: &str) {}
    async fn set_plugin_enabled(&self, input: SetPluginEnabledInput) -> PluginServiceResult<()>;
    async fn set_plugin_mcp_server_enabled(
        &self,
        input: SetPluginMcpServerEnabledInput,
    ) -> PluginServiceResult<()>;
    async fn remove_plugin(&self, input: RemovePluginInput) -> PluginServiceResult<()>;
    async fn reload_plugins(&self) -> PluginServiceResult<ReloadSummary>;
    async fn get_plugin_info(&self, input: GetPluginInfoInput) -> PluginServiceResult<PluginInfo>;
    async fn list_plugin_commands(&self) -> PluginServiceResult<Vec<PluginCommandDef>>;
    async fn check_updates(&self) -> PluginServiceResult<Vec<PluginUpdateStatus>>;
    async fn plugin_skill_roots(&self) -> PluginServiceResult<Vec<SkillRoot>>;
    async fn enabled_session_starts(&self) -> PluginServiceResult<Vec<EnabledPluginSessionStart>>;
    async fn enabled_mcp_servers(&self) -> PluginServiceResult<HashMap<String, McpServerConfig>>;
    async fn enabled_hooks(&self) -> PluginServiceResult<Vec<HookDef>>;
    fn on_did_reload(&self) -> Event<ReloadSummary>;
}

#[derive(Clone)]
pub struct PluginServiceHandle(pub Arc<dyn PluginServiceContract>);

impl Deref for PluginServiceHandle {
    type Target = dyn PluginServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for PluginServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const PLUGIN_SERVICE_ID: ServiceIdentifier<PluginServiceHandle> =
    ServiceIdentifier::new("pluginService");

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn service_identifier_and_input_wire_names_match_the_original() {
        assert_eq!(PLUGIN_SERVICE_ID.to_string(), "pluginService");
        assert_eq!(
            serde_json::to_value(SetPluginMcpServerEnabledInput {
                id: "demo".to_owned(),
                server: "local".to_owned(),
                enabled: true,
            })
            .unwrap(),
            json!({"id": "demo", "server": "local", "enabled": true})
        );
    }
}
