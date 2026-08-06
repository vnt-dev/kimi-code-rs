//! Built-in capability entries (kimi-cu, kimi-webbridge).
//!
//! Original: `packages/agent-core-v2/src/app/capability/entries/`.

pub mod context;
pub mod kimi_cu;
pub mod kimi_webbridge;
#[cfg(test)]
pub(crate) mod test_fakes;

pub use context::CapabilityEntryContext;
pub use kimi_cu::create_kimi_cu_entry;
pub use kimi_webbridge::create_kimi_webbridge_entry;

use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
};

use crate::app::{
    capability::types::{CapabilityStep, CapabilityStepState},
    plugin::PluginState,
};

/// Original: the per-entry `exists()` helper (node `access()`).
pub(crate) async fn path_exists(path: &Path) -> bool {
    tokio::fs::metadata(path).await.is_ok()
}

/// Original: the per-entry `executable()` helper (node `access(path, X_OK)`).
#[cfg(unix)]
pub(crate) async fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    tokio::fs::metadata(path)
        .await
        .map(|metadata| metadata.mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Windows has no execute bit; node `access(path, X_OK)` degrades to an
/// existence check there, and so do we.
#[cfg(not(unix))]
pub(crate) async fn is_executable(path: &Path) -> bool {
    path_exists(path).await
}

/// Original: node `mkdtemp(join(parent, prefix))`.
pub(crate) async fn mkdtemp_in(parent: &Path, prefix: &str) -> io::Result<PathBuf> {
    let dir = parent.join(format!("{prefix}{}", uuid::Uuid::new_v4().simple()));
    tokio::fs::create_dir(&dir).await?;
    Ok(dir)
}

/// Original: `Date.now()`.
pub(crate) fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Original: `PluginLayerConfig` — the agent-wiring plugin of a capability.
pub(crate) struct PluginLayerConfig {
    pub id: &'static str,
    pub zip_url: &'static str,
}

pub(crate) struct PluginLayerDetection {
    pub step: CapabilityStep,
    pub version: Option<String>,
}

// Original: detectPluginLayer() — one step describing the wiring plugin,
// with an MCP enable-gap detail when only part of its servers are enabled.
pub(crate) async fn detect_plugin_layer(
    ctx: &CapabilityEntryContext,
    config: &PluginLayerConfig,
    step_id: &str,
) -> Result<PluginLayerDetection, Box<dyn Error + Send + Sync>> {
    let installed = ctx.plugins.list_plugins().await?;
    let plugin = installed.iter().find(|candidate| candidate.id == config.id);
    let mcp_gap = plugin
        .filter(|plugin| plugin.enabled_mcp_server_count < plugin.mcp_server_count)
        .map(|plugin| {
            format!(
                "mcp {}/{} enabled",
                plugin.enabled_mcp_server_count, plugin.mcp_server_count
            )
        });
    let plugin_ok = plugin.is_some_and(|plugin| {
        plugin.enabled
            && plugin.state == PluginState::Ok
            && plugin.enabled_mcp_server_count == plugin.mcp_server_count
    });
    Ok(PluginLayerDetection {
        step: CapabilityStep {
            id: step_id.to_owned(),
            state: if plugin_ok {
                CapabilityStepState::Ok
            } else {
                CapabilityStepState::Missing
            },
            detail: mcp_gap.or_else(|| plugin.and_then(|plugin| plugin.version.clone())),
            optional: None,
        },
        version: plugin.and_then(|plugin| plugin.version.clone()),
    })
}
