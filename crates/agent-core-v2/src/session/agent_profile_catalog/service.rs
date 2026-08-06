//! Session-scoped merged agent profile catalog service.
//!
//! Original: `packages/agent-core-v2/src/session/sessionAgentProfileCatalog/sessionAgentProfileCatalogService.ts`.

use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex, RwLock},
};

use async_trait::async_trait;
use indexmap::IndexMap;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
        log::{LOG_SERVICE_ID, LogPayload, Logger},
    },
    app::{
        agent_file_catalog::{AgentProfileSourceContract, USER_FILE_AGENT_SOURCE_ID},
        agent_profile_catalog::{
            AGENT_PROFILE_CATALOG_SERVICE_ID, AgentProfile, AgentProfileCatalogHandle,
            DEFAULT_AGENT_PROFILE_NAME, MissingDefaultAgentProfile,
        },
    },
};

use super::{
    EXPLICIT_FILE_AGENT_SOURCE_ID, EXTRA_FILE_AGENT_SOURCE_ID, PROJECT_FILE_AGENT_SOURCE_ID,
    ProfileContributionWithPriority, SESSION_AGENT_PROFILE_CATALOG_ID,
    SessionAgentProfileCatalogContract, SessionAgentProfileCatalogError,
    SessionAgentProfileCatalogHandle, merge_agent_profiles,
};

struct SourceSlot {
    source: Arc<dyn AgentProfileSourceContract>,
    gate: AsyncMutex<()>,
}

struct CatalogState {
    contributions: HashMap<String, ProfileContributionWithPriority>,
    merged: IndexMap<String, Arc<AgentProfile>>,
}

enum ReadyState {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct CatalogLoadError(String);

impl fmt::Display for CatalogLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CatalogLoadError {}

pub struct SessionAgentProfileCatalogService {
    builtin: AgentProfileCatalogHandle,
    sources: Vec<Arc<SourceSlot>>,
    log: Arc<dyn Logger>,
    state: RwLock<CatalogState>,
    ready_state: Mutex<ReadyState>,
    ready_gate: AsyncMutex<()>,
    on_did_change_emitter: Arc<Emitter<String>>,
    disposables: DisposableStore,
}

impl SessionAgentProfileCatalogService {
    // Original: SessionAgentProfileCatalogService.constructor().
    pub fn new(
        builtin: AgentProfileCatalogHandle,
        user: Arc<dyn AgentProfileSourceContract>,
        extra: Arc<dyn AgentProfileSourceContract>,
        project: Arc<dyn AgentProfileSourceContract>,
        explicit: Arc<dyn AgentProfileSourceContract>,
        log: Arc<dyn Logger>,
    ) -> Arc<Self> {
        Self::from_sources(builtin, vec![user, extra, project, explicit], log)
    }

    fn from_sources(
        builtin: AgentProfileCatalogHandle,
        mut sources: Vec<Arc<dyn AgentProfileSourceContract>>,
        log: Arc<dyn Logger>,
    ) -> Arc<Self> {
        sources.sort_by_key(|source| source.priority());
        let source_slots = sources
            .into_iter()
            .map(|source| {
                Arc::new(SourceSlot {
                    source,
                    gate: AsyncMutex::new(()),
                })
            })
            .collect::<Vec<_>>();
        let emitter = Arc::new(Emitter::new());
        let service = Arc::new(Self {
            builtin,
            sources: source_slots,
            log,
            state: RwLock::new(CatalogState {
                contributions: HashMap::new(),
                merged: IndexMap::new(),
            }),
            ready_state: Mutex::new(ReadyState::Pending),
            ready_gate: AsyncMutex::new(()),
            on_did_change_emitter: Arc::clone(&emitter),
            disposables: DisposableStore::new(),
        });
        service.disposables.add(emitter);
        service.remerge();
        service.register_source_listeners();
        service.start_initial_load();
        service
    }

    fn register_source_listeners(self: &Arc<Self>) {
        for slot in &self.sources {
            let Some(event) = slot.source.on_did_change() else {
                continue;
            };
            let id = slot.source.id().to_owned();
            let weak = Arc::downgrade(self);
            let subscription = event.subscribe(move |_| {
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                    return;
                };
                let id = id.clone();
                runtime.spawn(async move {
                    if let Err(error) = service.reload_source(&id, true).await {
                        service.log.warn(
                            &format!("agent profile source \"{id}\" reload failed: {error}"),
                            None,
                        );
                    }
                });
            });
            self.disposables.add(subscription);
        }
    }

    fn start_initial_load(self: &Arc<Self>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(self);
        runtime.spawn(async move {
            if let Some(service) = weak.upgrade() {
                let _ = service.ensure_ready().await;
            }
        });
    }

    async fn ensure_ready(&self) -> Result<(), SessionAgentProfileCatalogError> {
        let _gate = self.ready_gate.lock().await;
        match &*self.ready_state.lock().unwrap() {
            ReadyState::Ready => return Ok(()),
            ReadyState::Failed(error) => return Err(Box::new(CatalogLoadError(error.clone()))),
            ReadyState::Pending => {}
        }
        let result = self.load_all().await;
        *self.ready_state.lock().unwrap() = match &result {
            Ok(()) => ReadyState::Ready,
            Err(error) => ReadyState::Failed(error.to_string()),
        };
        result
    }

    async fn reload_all(&self) -> Result<(), SessionAgentProfileCatalogError> {
        let _gate = self.ready_gate.lock().await;
        *self.ready_state.lock().unwrap() = ReadyState::Pending;
        let result = self.load_all().await;
        *self.ready_state.lock().unwrap() = match &result {
            Ok(()) => ReadyState::Ready,
            Err(error) => ReadyState::Failed(error.to_string()),
        };
        result
    }

    async fn load_all(&self) -> Result<(), SessionAgentProfileCatalogError> {
        let mut result = Ok(());
        for slot in &self.sources {
            if let Err(error) = self.load_source(slot, false).await {
                result = Err(error);
                break;
            }
        }
        self.remerge();
        result
    }

    async fn reload_source(
        &self,
        id: &str,
        fire_change: bool,
    ) -> Result<(), SessionAgentProfileCatalogError> {
        let Some(slot) = self.sources.iter().find(|slot| slot.source.id() == id) else {
            return Ok(());
        };
        self.load_source(slot, fire_change).await
    }

    async fn load_source(
        &self,
        slot: &SourceSlot,
        fire_change: bool,
    ) -> Result<(), SessionAgentProfileCatalogError> {
        let _gate = slot.gate.lock().await;
        let contribution = match slot.source.load().await {
            Ok(contribution) => contribution,
            Err(error) if slot.source.fatal() => return Err(error),
            Err(error) => {
                self.log.warn(
                    &format!(
                        "agent profile source \"{}\" load failed: {error}",
                        slot.source.id()
                    ),
                    Some(LogPayload::Error(crate::_base::log::LogEntryError {
                        message: error.to_string(),
                        stack: None,
                    })),
                );
                return Ok(());
            }
        };
        self.state.write().unwrap().contributions.insert(
            slot.source.id().to_owned(),
            ProfileContributionWithPriority {
                contribution,
                priority: slot.source.priority(),
            },
        );
        if fire_change {
            self.remerge();
            self.on_did_change_emitter
                .fire(&slot.source.id().to_owned());
        }
        Ok(())
    }

    fn remerge(&self) {
        let contributions = self
            .state
            .read()
            .unwrap()
            .contributions
            .values()
            .map(|value| ProfileContributionWithPriority {
                contribution: value.contribution.clone(),
                priority: value.priority,
            })
            .collect::<Vec<_>>();
        let log = Arc::clone(&self.log);
        let merged = merge_agent_profiles(self.builtin.list(), contributions, move |message| {
            log.warn(message, None);
        });
        self.state.write().unwrap().merged = merged;
    }
}

#[async_trait]
impl SessionAgentProfileCatalogContract for SessionAgentProfileCatalogService {
    async fn ready(&self) -> Result<(), SessionAgentProfileCatalogError> {
        self.ensure_ready().await
    }
    fn on_did_change(&self) -> Event<String> {
        self.on_did_change_emitter.event()
    }
    fn get(&self, name: &str) -> Option<Arc<AgentProfile>> {
        self.state.read().unwrap().merged.get(name).cloned()
    }
    fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile> {
        self.get(DEFAULT_AGENT_PROFILE_NAME)
            .ok_or(MissingDefaultAgentProfile)
    }
    fn list(&self) -> Vec<Arc<AgentProfile>> {
        self.state
            .read()
            .unwrap()
            .merged
            .values()
            .cloned()
            .collect()
    }
    async fn load(&self) -> Result<(), SessionAgentProfileCatalogError> {
        self.ensure_ready().await
    }
    async fn reload(&self) -> Result<(), SessionAgentProfileCatalogError> {
        self.reload_all().await?;
        self.on_did_change_emitter.fire(&"catalog".into());
        Ok(())
    }
}

impl Disposable for SessionAgentProfileCatalogService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_session_agent_profile_catalog() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_AGENT_PROFILE_CATALOG_ID,
        SyncDescriptor::new(|accessor| {
            let builtin = accessor.get(AGENT_PROFILE_CATALOG_SERVICE_ID)?;
            let user = accessor.get(USER_FILE_AGENT_SOURCE_ID)?;
            let extra = accessor.get(EXTRA_FILE_AGENT_SOURCE_ID)?;
            let project = accessor.get(PROJECT_FILE_AGENT_SOURCE_ID)?;
            let explicit = accessor.get(EXPLICIT_FILE_AGENT_SOURCE_ID)?;
            let log = accessor.get(LOG_SERVICE_ID)?;
            let service: Arc<dyn SessionAgentProfileCatalogContract> =
                SessionAgentProfileCatalogService::new(
                    (*builtin).clone(),
                    user.0.clone(),
                    extra.0.clone(),
                    project.0.clone(),
                    explicit.0.clone(),
                    log.0.clone(),
                );
            Ok(SessionAgentProfileCatalogHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionAgentProfileCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    };

    use crate::{
        _base::{
            di::lifecycle::DisposeResult,
            event::Event,
            log::{LogContext, LogPayload},
        },
        app::{
            agent_file_catalog::{AgentProfileContribution, AgentProfileSourceError},
            agent_profile_catalog::{AgentProfileCatalogContract, AgentSystemPrompt},
        },
    };

    use super::*;

    #[derive(Debug)]
    struct TestError;
    impl fmt::Display for TestError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("test source failure")
        }
    }
    impl Error for TestError {}

    struct StaticSource {
        id: &'static str,
        priority: i32,
        fatal: bool,
        contribution: RwLock<AgentProfileContribution>,
        fail: AtomicBool,
    }

    #[async_trait]
    impl AgentProfileSourceContract for StaticSource {
        fn id(&self) -> &str {
            self.id
        }
        fn priority(&self) -> i32 {
            self.priority
        }
        fn fatal(&self) -> bool {
            self.fatal
        }
        fn on_did_change(&self) -> Option<Event<()>> {
            None
        }
        async fn load(&self) -> Result<AgentProfileContribution, AgentProfileSourceError> {
            if self.fail.load(Ordering::Relaxed) {
                return Err(Box::new(TestError));
            }
            Ok(self.contribution.read().unwrap().clone())
        }
    }

    impl Disposable for StaticSource {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    struct StaticBuiltin(Vec<Arc<AgentProfile>>);
    impl AgentProfileCatalogContract for StaticBuiltin {
        fn get(&self, name: &str) -> Option<Arc<AgentProfile>> {
            self.0.iter().find(|profile| profile.name == name).cloned()
        }
        fn get_default(&self) -> Result<Arc<AgentProfile>, MissingDefaultAgentProfile> {
            self.get(DEFAULT_AGENT_PROFILE_NAME)
                .ok_or(MissingDefaultAgentProfile)
        }
        fn list(&self) -> Vec<Arc<AgentProfile>> {
            self.0.clone()
        }
    }

    struct SilentLogger;
    impl Logger for SilentLogger {
        fn error(&self, _: &str, _: Option<LogPayload>) {}
        fn warn(&self, _: &str, _: Option<LogPayload>) {}
        fn info(&self, _: &str, _: Option<LogPayload>) {}
        fn debug(&self, _: &str, _: Option<LogPayload>) {}
        fn child(&self, _: LogContext) -> Arc<dyn Logger> {
            Arc::new(Self)
        }
    }

    fn profile(name: &str, prompt: &str, override_profile: bool) -> Arc<AgentProfile> {
        let prompt: AgentSystemPrompt = Arc::new({
            let prompt = prompt.to_owned();
            move |_| prompt.clone()
        });
        Arc::new(AgentProfile {
            name: name.into(),
            description: None,
            when_to_use: None,
            is_override: Some(override_profile),
            tools: None,
            disallowed_tools: None,
            subagents: None,
            model: None,
            system_prompt: prompt,
            prompt_prefix: None,
            summary_policy: None,
        })
    }

    fn source(
        id: &'static str,
        priority: i32,
        fatal: bool,
        profiles: Vec<Arc<AgentProfile>>,
    ) -> Arc<StaticSource> {
        Arc::new(StaticSource {
            id,
            priority,
            fatal,
            contribution: RwLock::new(AgentProfileContribution {
                profiles,
                skipped: None,
                scanned_roots: None,
            }),
            fail: AtomicBool::new(false),
        })
    }

    #[tokio::test]
    async fn ready_and_reload_preserve_nonfatal_state_and_surface_fatal_errors() {
        let user = source("user", 10, false, vec![profile("agent", "ignored", false)]);
        let extra = source("extra", 20, false, vec![profile("review", "extra", false)]);
        let project = source(
            "project",
            30,
            false,
            vec![profile("agent", "project", true)],
        );
        let explicit = source("explicit", 40, true, Vec::new());
        let builtin: Arc<dyn AgentProfileCatalogContract> =
            Arc::new(StaticBuiltin(vec![profile("agent", "builtin", false)]));
        let service = SessionAgentProfileCatalogService::from_sources(
            AgentProfileCatalogHandle(builtin),
            vec![
                user.clone(),
                extra.clone(),
                project.clone(),
                explicit.clone(),
            ],
            Arc::new(SilentLogger),
        );

        service.ready().await.unwrap();
        assert_eq!(
            service
                .get_default()
                .unwrap()
                .render_system_prompt(&Default::default()),
            "project"
        );
        assert_eq!(
            service
                .get("review")
                .unwrap()
                .render_system_prompt(&Default::default()),
            "extra"
        );

        extra.fail.store(true, Ordering::Relaxed);
        service.reload().await.unwrap();
        assert_eq!(
            service
                .get("review")
                .unwrap()
                .render_system_prompt(&Default::default()),
            "extra"
        );

        explicit.fail.store(true, Ordering::Relaxed);
        assert!(service.reload().await.is_err());
        assert_eq!(
            service
                .get_default()
                .unwrap()
                .render_system_prompt(&Default::default()),
            "project"
        );

        explicit.fail.store(false, Ordering::Relaxed);
        extra.fail.store(false, Ordering::Relaxed);
        service.reload().await.unwrap();
        service.dispose().unwrap();
    }
}
