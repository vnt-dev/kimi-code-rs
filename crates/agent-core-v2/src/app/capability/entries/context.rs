//! Shared context injected into capability entries. Every field is
//! constructor-wired by `CapabilityService`; tests substitute fakes
//! (temp dirs, fake fetch, fake plugin service) rather than touching the
//! host.
//!
//! Original: `packages/agent-core-v2/src/app/capability/entries/context.ts`.

use std::{path::PathBuf, sync::Arc, time::Duration};

use crate::{
    app::{
        capability::host::{FetchLike, ReqwestFetch},
        plugin::PluginServiceHandle,
    },
    os::interface::host_process::HostProcessServiceHandle,
};

#[derive(Clone)]
pub struct CapabilityEntryContext {
    pub platform: String,
    pub arch: String,
    pub kimi_home_dir: PathBuf,
    pub user_home_dir: PathBuf,
    pub plugins: PluginServiceHandle,
    pub host_process: HostProcessServiceHandle,
    pub fetch_impl: Option<Arc<dyn FetchLike>>,
    pub applications_dir: Option<PathBuf>,
    pub webbridge_base_url: Option<String>,
    pub detect_probe_timeout: Option<Duration>,
    pub command_timeout: Option<Duration>,
}

impl CapabilityEntryContext {
    /// Original: `ctx.fetchImpl ?? fetch`.
    pub fn fetch_impl_or_default(&self) -> Arc<dyn FetchLike> {
        self.fetch_impl
            .clone()
            .unwrap_or_else(|| Arc::new(ReqwestFetch))
    }
}
