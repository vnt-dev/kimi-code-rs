use std::{
    error::Error,
    sync::{Arc, Mutex, RwLock},
};

use async_trait::async_trait;
use tokio::{sync::OnceCell, task::JoinHandle};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::Event,
    },
    agent::external_hooks::{HOOKS_SECTION, HookDef, HookDefConfig, register_hooks_config_section},
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        plugin::{PLUGIN_SERVICE_ID, PluginServiceHandle, ReloadSummary},
    },
    os::interface::host_process::{
        HOST_PROCESS_SERVICE_ID, HostProcessService, HostProcessServiceHandle,
    },
};

use super::{
    contract::{
        EXTERNAL_HOOKS_RUNNER_SERVICE_ID, ExternalHooksRunnerServiceContract,
        ExternalHooksRunnerServiceHandle, ExternalHooksRunnerTriggerArgs,
    },
    runner::{HookRunCallbacks, HooksByEvent, block_decision, index_hooks, run_matched_hooks},
};

type HookSourceError = Box<dyn Error + Send + Sync>;

#[async_trait]
pub trait ExternalHookDefinitionSource: Send + Sync {
    async fn load(&self) -> Result<Vec<HookDef>, HookSourceError>;
    fn on_did_reload(&self) -> Event<ReloadSummary>;
}

struct ConfigPluginHookSource {
    config: ConfigServiceHandle,
    plugins: PluginServiceHandle,
}

#[async_trait]
impl ExternalHookDefinitionSource for ConfigPluginHookSource {
    async fn load(&self) -> Result<Vec<HookDef>, HookSourceError> {
        self.config.ready().await?;
        let configured = self
            .config
            .get(HOOKS_SECTION)
            .map(serde_json::from_value::<Vec<HookDefConfig>>)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|hook| HookDef {
                event: hook.event,
                matcher: hook.matcher,
                command: hook.command,
                timeout: hook.timeout,
                cwd: None,
                env: None,
            });
        let plugin_hooks = self.plugins.enabled_hooks().await?;
        Ok(configured.chain(plugin_hooks).collect())
    }

    fn on_did_reload(&self) -> Event<ReloadSummary> {
        self.plugins.on_did_reload()
    }
}

pub struct ExternalHooksRunnerService {
    source: Arc<dyn ExternalHookDefinitionSource>,
    cwd: String,
    host_process: Arc<dyn HostProcessService>,
    callbacks: HookRunCallbacks,
    by_event: RwLock<HooksByEvent>,
    initialized: OnceCell<()>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    disposables: DisposableStore,
}

impl ExternalHooksRunnerService {
    // Original:
    //   packages/agent-core-v2/src/app/externalHooksRunner/externalHooksRunnerService.ts
    //   ExternalHooksRunnerService.constructor()
    pub fn new(
        source: Arc<dyn ExternalHookDefinitionSource>,
        cwd: String,
        host_process: Arc<dyn HostProcessService>,
        callbacks: HookRunCallbacks,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            source,
            cwd,
            host_process,
            callbacks,
            by_event: RwLock::new(HooksByEvent::new()),
            initialized: OnceCell::new(),
            tasks: Mutex::new(Vec::new()),
            disposables: DisposableStore::new(),
        });
        let weak = Arc::downgrade(&service);
        service
            .disposables
            .add(service.source.on_did_reload().subscribe(move |_| {
                if let Some(service) = weak.upgrade() {
                    service.spawn_reload();
                }
            }));
        service.spawn_initial_load();
        service
    }

    pub fn from_services(
        config: ConfigServiceHandle,
        plugins: PluginServiceHandle,
        bootstrap: BootstrapServiceHandle,
        host_process: HostProcessServiceHandle,
        callbacks: HookRunCallbacks,
    ) -> Arc<Self> {
        Self::new(
            Arc::new(ConfigPluginHookSource { config, plugins }),
            bootstrap.cwd().to_string_lossy().into_owned(),
            Arc::clone(&host_process.0),
            callbacks,
        )
    }

    // Original: ExternalHooksRunnerService.summary getter.
    pub fn summary(&self) -> std::collections::HashMap<String, usize> {
        self.by_event
            .read()
            .unwrap()
            .iter()
            .map(|(event, hooks)| (event.clone(), hooks.len()))
            .collect()
    }

    fn spawn_initial_load(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let service = Arc::clone(self);
        self.tasks.lock().unwrap().push(runtime.spawn(async move {
            service.ready().await;
        }));
    }

    fn spawn_reload(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let service = Arc::clone(self);
        self.tasks.lock().unwrap().push(runtime.spawn(async move {
            service.load_safe().await;
        }));
    }

    async fn ready(&self) {
        self.initialized
            .get_or_init(|| async {
                self.load_safe().await;
            })
            .await;
    }

    // Original: ExternalHooksRunnerService.loadSafe()/reloadSafe().
    async fn load_safe(&self) {
        if let Ok(hooks) = self.source.load().await {
            *self.by_event.write().unwrap() = index_hooks(&hooks);
        }
    }
}

#[async_trait]
impl ExternalHooksRunnerServiceContract for ExternalHooksRunnerService {
    // Original: ExternalHooksRunnerService.trigger()/triggerInner(). All
    // dependency failures have already been converted to an empty/stale index.
    async fn trigger(
        &self,
        event: &str,
        mut args: ExternalHooksRunnerTriggerArgs,
    ) -> Vec<crate::agent::external_hooks::HookResult> {
        self.ready().await;
        if args.cwd.is_none() {
            args.cwd = Some(self.cwd.clone());
        }
        let by_event = self.by_event.read().unwrap().clone();
        run_matched_hooks(
            self.host_process.as_ref(),
            &by_event,
            event,
            &args,
            &self.callbacks,
        )
        .await
    }

    // Original: ExternalHooksRunnerService.triggerBlock().
    async fn trigger_block(
        &self,
        event: &str,
        args: ExternalHooksRunnerTriggerArgs,
    ) -> Option<crate::agent::external_hooks::HookBlockDecision> {
        block_decision(event, &self.trigger(event, args).await)
    }

    // Original: ExternalHooksRunnerService.fireAndForgetTrigger(). The source
    // returns the same fail-open promise; callers may choose not to await it.
    async fn fire_and_forget_trigger(
        &self,
        event: &str,
        args: ExternalHooksRunnerTriggerArgs,
    ) -> Vec<crate::agent::external_hooks::HookResult> {
        self.trigger(event, args).await
    }
}

impl Disposable for ExternalHooksRunnerService {
    fn dispose(&self) -> DisposeResult {
        let subscription_result = self.disposables.dispose();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        subscription_result
    }
}

// Original: registerScopedService(... ExternalHooksRunnerService ...).
pub fn register_external_hooks_runner_service() {
    register_hooks_config_section();
    register_scoped_service(
        LifecycleScope::App,
        EXTERNAL_HOOKS_RUNNER_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let config = accessor.get(CONFIG_SERVICE_ID)?;
            let plugins = accessor.get(PLUGIN_SERVICE_ID)?;
            let bootstrap = accessor.get(BOOTSTRAP_SERVICE_ID)?;
            let host_process = accessor.get(HOST_PROCESS_SERVICE_ID)?;
            let service: Arc<dyn ExternalHooksRunnerServiceContract> =
                ExternalHooksRunnerService::from_services(
                    (*config).clone(),
                    (*plugins).clone(),
                    (*bootstrap).clone(),
                    (*host_process).clone(),
                    HookRunCallbacks::default(),
                );
            Ok(ExternalHooksRunnerServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "externalHooksRunner",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use crate::{
        _base::{di::lifecycle::DisposableHandle, event::Emitter},
        agent::external_hooks::HookEventType,
        os::backends::node_local::host_process_service::LocalHostProcessService,
    };

    use super::*;

    struct StubSource {
        hooks: Mutex<Vec<HookDef>>,
        fail: AtomicBool,
        loads: AtomicUsize,
        reloaded: Arc<Emitter<ReloadSummary>>,
    }

    impl StubSource {
        fn new(hooks: Vec<HookDef>) -> Arc<Self> {
            Arc::new(Self {
                hooks: Mutex::new(hooks),
                fail: AtomicBool::new(false),
                loads: AtomicUsize::new(0),
                reloaded: Arc::new(Emitter::new()),
            })
        }

        fn reload(&self, hooks: Vec<HookDef>, fail: bool) {
            *self.hooks.lock().unwrap() = hooks;
            self.fail.store(fail, Ordering::SeqCst);
            self.reloaded.fire(&ReloadSummary::default());
        }
    }

    #[async_trait]
    impl ExternalHookDefinitionSource for StubSource {
        async fn load(&self) -> Result<Vec<HookDef>, HookSourceError> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("load failed".into());
            }
            Ok(self.hooks.lock().unwrap().clone())
        }

        fn on_did_reload(&self) -> Event<ReloadSummary> {
            self.reloaded.event()
        }
    }

    fn hook(event: HookEventType, command: &str) -> HookDef {
        HookDef {
            event,
            matcher: None,
            command: command.into(),
            timeout: Some(1),
            cwd: None,
            env: None,
        }
    }

    async fn wait_for_loads(source: &StubSource, count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while source.loads.load(Ordering::SeqCst) < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn waits_for_initial_load_reloads_and_keeps_stale_index_on_failure() {
        let source = StubSource::new(vec![hook(HookEventType::Stop, "printf stop")]);
        let source_handle: Arc<dyn ExternalHookDefinitionSource> = source.clone();
        let host: Arc<dyn HostProcessService> = Arc::new(LocalHostProcessService::default());
        let service = ExternalHooksRunnerService::new(
            source_handle,
            "/workspace".into(),
            host,
            HookRunCallbacks::default(),
        );

        assert!(
            service
                .trigger("Missing", Default::default())
                .await
                .is_empty()
        );
        assert_eq!(service.summary()["Stop"], 1);
        source.reload(
            vec![
                hook(HookEventType::Notification, "printf one"),
                hook(HookEventType::Notification, "printf two"),
            ],
            false,
        );
        wait_for_loads(&source, 2).await;
        assert_eq!(service.summary()["Notification"], 2);

        source.reload(vec![hook(HookEventType::PreCompact, "printf bad")], true);
        wait_for_loads(&source, 3).await;
        assert_eq!(service.summary()["Notification"], 2);
        assert!(!service.summary().contains_key("PreCompact"));

        let disposable: DisposableHandle = service;
        disposable.dispose().unwrap();
    }
}
