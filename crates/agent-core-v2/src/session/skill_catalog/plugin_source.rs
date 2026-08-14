//! Plugin-provided skill contribution source.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/pluginSkillSource.ts`.

use std::{ops::Deref, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    app::{
        plugin::{PLUGIN_SERVICE_ID, PluginServiceHandle},
        skill_catalog::{
            SKILL_DISCOVERY_SERVICE_ID, SKILL_SOURCE_PRIORITY, SkillContribution,
            SkillDiscoveryHandle, SkillSourceContract, SkillSourceError, SkillSourceResult,
        },
    },
};

pub struct PluginSkillSource {
    discovery: SkillDiscoveryHandle,
    plugins: PluginServiceHandle,
}

impl PluginSkillSource {
    pub fn new(discovery: SkillDiscoveryHandle, plugins: PluginServiceHandle) -> Self {
        Self { discovery, plugins }
    }
}

#[async_trait]
impl SkillSourceContract for PluginSkillSource {
    fn id(&self) -> &str {
        PLUGIN_SKILL_SOURCE_ID
    }

    fn priority(&self) -> i32 {
        SKILL_SOURCE_PRIORITY.plugin
    }

    fn on_did_change(&self) -> Option<crate::_base::event::Event<()>> {
        Some(self.plugins.on_did_reload().map(|_| ()))
    }

    // Original: PluginSkillSource.load(). Root lookup completes before
    // discovery begins, preserving the source's sequential await order.
    async fn load(&self) -> SkillSourceResult<SkillContribution> {
        let roots = self
            .plugins
            .plugin_skill_roots()
            .await
            .map_err(SkillSourceError::Plugin)?;
        let result = self.discovery.discover(&roots).await;
        Ok(SkillContribution {
            skills: result.skills,
            skipped: Some(result.skipped),
            scanned_roots: Some(result.scanned_roots),
        })
    }
}

#[derive(Clone)]
pub struct PluginSkillSourceHandle(pub Arc<dyn SkillSourceContract>);

impl Deref for PluginSkillSourceHandle {
    type Target = dyn SkillSourceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const PLUGIN_SKILL_SOURCE_ID: &str = "plugin";

pub const PLUGIN_SKILL_SOURCE_SERVICE_ID: ServiceIdentifier<PluginSkillSourceHandle> =
    ServiceIdentifier::new("pluginSkillSource");

pub fn register_plugin_skill_source() {
    register_scoped_service(
        LifecycleScope::Session,
        PLUGIN_SKILL_SOURCE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let discovery = accessor.get(SKILL_DISCOVERY_SERVICE_ID)?;
            let plugins = accessor.get(PLUGIN_SERVICE_ID)?;
            let source: Arc<dyn SkillSourceContract> = Arc::new(PluginSkillSource::new(
                (*discovery).clone(),
                (*plugins).clone(),
            ));
            Ok(PluginSkillSourceHandle(source))
        }),
        InstantiationType::Eager,
        "sessionSkillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io,
    };
    use std::sync::{atomic::{AtomicUsize, Ordering}};
    use parking_lot::Mutex;

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{Disposable, DisposeResult},
            event::{Emitter, Event},
        },
        agent::{external_hooks::HookDef, mcp::McpServerConfig},
        app::{
            plugin::{
                EnabledPluginSessionStart, GetPluginInfoInput, InstallPluginInput,
                PluginCommandDef, PluginInfo, PluginInstallOperation, PluginServiceContract,
                PluginServiceResult, PluginSummary, PluginUpdateStatus, ReloadSummary,
                RemovePluginInput, SetPluginEnabledInput, SetPluginMcpServerEnabledInput,
            },
            skill_catalog::{SkillDiscoveryContract, SkillDiscoveryResult, SkillRoot, SkillSource},
        },
    };

    struct StubPlugins {
        roots: Vec<SkillRoot>,
        root_error: bool,
        reloaded: Emitter<ReloadSummary>,
    }

    impl StubPlugins {
        fn unsupported<T>() -> PluginServiceResult<T> {
            Err(Box::new(io::Error::other("not used by PluginSkillSource")))
        }
    }

    impl Disposable for StubPlugins {
        fn dispose(&self) -> DisposeResult {
            self.reloaded.dispose()
        }
    }

    #[async_trait]
    impl PluginServiceContract for StubPlugins {
        async fn list_plugins(&self) -> PluginServiceResult<Vec<PluginSummary>> {
            Self::unsupported()
        }
        async fn install_plugin(
            &self,
            _: InstallPluginInput,
        ) -> PluginServiceResult<PluginSummary> {
            Self::unsupported()
        }
        async fn set_plugin_enabled(&self, _: SetPluginEnabledInput) -> PluginServiceResult<()> {
            Self::unsupported()
        }
        async fn set_plugin_mcp_server_enabled(
            &self,
            _: SetPluginMcpServerEnabledInput,
        ) -> PluginServiceResult<()> {
            Self::unsupported()
        }
        async fn remove_plugin(&self, _: RemovePluginInput) -> PluginServiceResult<()> {
            Self::unsupported()
        }
        async fn install_plugin_in_background(
            self: Arc<Self>,
            _: InstallPluginInput,
            _: String,
        ) -> PluginServiceResult<()> {
            Self::unsupported()
        }
        fn plugin_install_progress(&self, _: &str) -> Option<PluginInstallOperation> {
            None
        }
        async fn reload_plugins(&self) -> PluginServiceResult<ReloadSummary> {
            Self::unsupported()
        }
        async fn get_plugin_info(&self, _: GetPluginInfoInput) -> PluginServiceResult<PluginInfo> {
            Self::unsupported()
        }
        async fn list_plugin_commands(&self) -> PluginServiceResult<Vec<PluginCommandDef>> {
            Self::unsupported()
        }
        async fn check_updates(&self) -> PluginServiceResult<Vec<PluginUpdateStatus>> {
            Self::unsupported()
        }
        async fn plugin_skill_roots(&self) -> PluginServiceResult<Vec<SkillRoot>> {
            if self.root_error {
                return Err(Box::new(io::Error::other("plugin roots unavailable")));
            }
            Ok(self.roots.clone())
        }
        async fn enabled_session_starts(
            &self,
        ) -> PluginServiceResult<Vec<EnabledPluginSessionStart>> {
            Self::unsupported()
        }
        async fn enabled_mcp_servers(
            &self,
        ) -> PluginServiceResult<HashMap<String, McpServerConfig>> {
            Self::unsupported()
        }
        async fn enabled_hooks(&self) -> PluginServiceResult<Vec<HookDef>> {
            Self::unsupported()
        }
        fn on_did_reload(&self) -> Event<ReloadSummary> {
            self.reloaded.event()
        }
    }

    #[derive(Default)]
    struct RecordingDiscovery {
        roots: Mutex<Vec<SkillRoot>>,
    }

    #[async_trait]
    impl SkillDiscoveryContract for RecordingDiscovery {
        async fn discover(&self, roots: &[SkillRoot]) -> SkillDiscoveryResult {
            *self.roots.lock() = roots.to_vec();
            SkillDiscoveryResult {
                scanned_roots: roots.iter().map(|root| root.path.clone()).collect(),
                ..SkillDiscoveryResult::default()
            }
        }
    }

    #[tokio::test]
    async fn discovers_plugin_roots_and_reemits_plugin_reload() {
        let plugins = Arc::new(StubPlugins {
            roots: vec![SkillRoot {
                path: "/plugins/demo/skills".into(),
                source: SkillSource::Project,
                plugin: None,
            }],
            root_error: false,
            reloaded: Emitter::new(),
        });
        let discovery = Arc::new(RecordingDiscovery::default());
        let source = PluginSkillSource::new(
            SkillDiscoveryHandle(discovery.clone()),
            PluginServiceHandle(plugins.clone()),
        );
        let changes = Arc::new(AtomicUsize::new(0));
        let changes_for_listener = Arc::clone(&changes);
        let subscription = source.on_did_change().unwrap().subscribe(move |_| {
            changes_for_listener.fetch_add(1, Ordering::Relaxed);
        });

        let contribution = source.load().await.unwrap();

        assert_eq!(source.id(), "plugin");
        assert_eq!(source.priority(), SKILL_SOURCE_PRIORITY.plugin);
        assert_eq!(
            contribution.scanned_roots,
            Some(vec!["/plugins/demo/skills".into()])
        );
        assert_eq!(
            discovery.roots.lock().as_slice(),
            plugins.roots.as_slice()
        );
        plugins.reloaded.fire(&ReloadSummary::default());
        assert_eq!(changes.load(Ordering::Relaxed), 1);
        subscription.dispose().unwrap();
        assert_eq!(PLUGIN_SKILL_SOURCE_ID, "plugin");
        assert_eq!(
            PLUGIN_SKILL_SOURCE_SERVICE_ID.to_string(),
            "pluginSkillSource"
        );
    }

    #[tokio::test]
    async fn preserves_plugin_root_lookup_failures() {
        let source = PluginSkillSource::new(
            SkillDiscoveryHandle(Arc::new(RecordingDiscovery::default())),
            PluginServiceHandle(Arc::new(StubPlugins {
                roots: Vec::new(),
                root_error: true,
                reloaded: Emitter::new(),
            })),
        );

        assert!(matches!(
            source.load().await,
            Err(SkillSourceError::Plugin(_))
        ));
    }
}
