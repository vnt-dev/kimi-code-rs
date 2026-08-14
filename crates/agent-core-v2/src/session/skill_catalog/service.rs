//! Session-scoped merged skill catalog service.
//!
//! Original: `packages/agent-core-v2/src/session/sessionSkillCatalog/skillCatalogService.ts`.

use parking_lot::Mutex;
use parking_lot::RwLock;
use std::sync::Arc;

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
    },
    app::skill_catalog::{
        BUILTIN_SKILL_SOURCE_ID, InMemorySkillCatalog, RegisterSkillOptions, SkillCatalogContract,
        SkillContribution, SkillSourceContract, SkillSourceError, SkillSourceResult,
        USER_FILE_SKILL_SOURCE_ID,
    },
};

use super::{
    EXPLICIT_FILE_SKILL_SOURCE_ID, EXTRA_FILE_SKILL_SOURCE_ID, PLUGIN_SKILL_SOURCE_SERVICE_ID,
    SESSION_SKILL_CATALOG_ID, SessionSkillCatalogContract, SessionSkillCatalogHandle,
    SkillCatalogSinkContract, SkillCatalogSinkOptions, WORKSPACE_FILE_SKILL_SOURCE_ID,
};

struct SourceSlot {
    source: Arc<dyn SkillSourceContract>,
    gate: AsyncMutex<()>,
}

#[derive(Clone)]
struct ContributionWithPriority {
    contribution: SkillContribution,
    priority: i32,
}

enum ReadyState {
    Pending,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct InitialLoadError(String);

impl std::fmt::Display for InitialLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InitialLoadError {}

pub struct SessionSkillCatalogService {
    sources: Vec<Arc<SourceSlot>>,
    contributions: RwLock<IndexMap<String, ContributionWithPriority>>,
    merged: RwLock<Arc<dyn SkillCatalogContract>>,
    ready_state: Mutex<ReadyState>,
    ready_gate: AsyncMutex<()>,
    on_did_change_emitter: Arc<Emitter<String>>,
    disposables: DisposableStore,
}

impl SessionSkillCatalogService {
    // Original: SessionSkillCatalogService.constructor().
    pub fn new(
        builtin: Arc<dyn SkillSourceContract>,
        user: Arc<dyn SkillSourceContract>,
        explicit: Arc<dyn SkillSourceContract>,
        extra: Arc<dyn SkillSourceContract>,
        workspace: Arc<dyn SkillSourceContract>,
        plugin: Arc<dyn SkillSourceContract>,
    ) -> Arc<Self> {
        Self::from_sources(vec![builtin, user, explicit, extra, workspace, plugin])
    }

    fn from_sources(mut sources: Vec<Arc<dyn SkillSourceContract>>) -> Arc<Self> {
        sources.sort_by_key(|source| source.priority());
        let sources = sources
            .into_iter()
            .map(|source| {
                Arc::new(SourceSlot {
                    source,
                    gate: AsyncMutex::new(()),
                })
            })
            .collect();
        let emitter = Arc::new(Emitter::new());
        let service = Arc::new(Self {
            sources,
            contributions: RwLock::new(IndexMap::new()),
            merged: RwLock::new(Arc::new(InMemorySkillCatalog::default())),
            ready_state: Mutex::new(ReadyState::Pending),
            ready_gate: AsyncMutex::new(()),
            on_did_change_emitter: Arc::clone(&emitter),
            disposables: DisposableStore::new(),
        });
        service.disposables.add(emitter);
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
                    let _ = service.reload_source(&id, true).await;
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

    async fn ensure_ready(&self) -> SkillSourceResult<()> {
        let _gate = self.ready_gate.lock().await;
        match &*self.ready_state.lock() {
            ReadyState::Ready => return Ok(()),
            ReadyState::Failed(error) => {
                return Err(SkillSourceError::Cached(Box::new(InitialLoadError(
                    error.clone(),
                ))));
            }
            ReadyState::Pending => {}
        }
        let result = self.load_all().await;
        *self.ready_state.lock() = match &result {
            Ok(()) => ReadyState::Ready,
            Err(error) => ReadyState::Failed(error.to_string()),
        };
        result
    }

    async fn load_all(&self) -> SkillSourceResult<()> {
        for slot in &self.sources {
            self.load_source(slot, false).await?;
        }
        self.remerge();
        Ok(())
    }

    async fn reload_source(&self, id: &str, fire_change: bool) -> SkillSourceResult<()> {
        let Some(slot) = self.sources.iter().find(|slot| slot.source.id() == id) else {
            return Ok(());
        };
        self.load_source(slot, fire_change).await
    }

    async fn load_source(&self, slot: &SourceSlot, fire_change: bool) -> SkillSourceResult<()> {
        let _gate = slot.gate.lock().await;
        let contribution = slot.source.load().await?;
        self.contributions.write().insert(
            slot.source.id().to_owned(),
            ContributionWithPriority {
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
        let mut contributions = self
            .contributions
            .read()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        // `sort_by_key` is stable, retaining the original Map insertion order
        // for equal-priority sources such as user and explicit skills.
        contributions.sort_by_key(|entry| entry.priority);
        let mut merged = InMemorySkillCatalog::default();
        for entry in contributions {
            for skill in entry.contribution.skills {
                merged.register(skill, RegisterSkillOptions { replace: true });
            }
            merged.add_roots(
                entry
                    .contribution
                    .scanned_roots
                    .as_deref()
                    .unwrap_or_default(),
            );
            merged.record_skipped(entry.contribution.skipped.as_deref().unwrap_or_default());
        }
        *self.merged.write() = Arc::new(merged);
    }
}

#[async_trait]
impl SessionSkillCatalogContract for SessionSkillCatalogService {
    fn catalog(&self) -> Arc<dyn SkillCatalogContract> {
        self.merged.read().clone()
    }

    fn on_did_change(&self) -> Event<String> {
        self.on_did_change_emitter.event()
    }

    async fn ready(&self) -> SkillSourceResult<()> {
        self.ensure_ready().await
    }

    async fn load(&self) -> SkillSourceResult<()> {
        self.ensure_ready().await
    }

    async fn reload(&self) -> SkillSourceResult<()> {
        self.load_all().await?;
        self.on_did_change_emitter.fire(&"catalog".into());
        Ok(())
    }
}

impl SkillCatalogSinkContract for SessionSkillCatalogService {
    fn set(&self, id: &str, contribution: SkillContribution, options: SkillCatalogSinkOptions) {
        self.contributions.write().insert(
            id.to_owned(),
            ContributionWithPriority {
                contribution,
                priority: options.priority,
            },
        );
        self.remerge();
        self.on_did_change_emitter.fire(&id.to_owned());
    }

    fn remove(&self, id: &str) {
        self.contributions.write().shift_remove(id);
        self.remerge();
        self.on_did_change_emitter.fire(&id.to_owned());
    }
}

impl Disposable for SessionSkillCatalogService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_session_skill_catalog() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_SKILL_CATALOG_ID,
        SyncDescriptor::new(|accessor| {
            let builtin = accessor.get(BUILTIN_SKILL_SOURCE_ID)?;
            let user = accessor.get(USER_FILE_SKILL_SOURCE_ID)?;
            let explicit = accessor.get(EXPLICIT_FILE_SKILL_SOURCE_ID)?;
            let extra = accessor.get(EXTRA_FILE_SKILL_SOURCE_ID)?;
            let workspace = accessor.get(WORKSPACE_FILE_SKILL_SOURCE_ID)?;
            let plugin = accessor.get(PLUGIN_SKILL_SOURCE_SERVICE_ID)?;
            let service: Arc<dyn SessionSkillCatalogContract> = SessionSkillCatalogService::new(
                builtin.0.clone(),
                user.0.clone(),
                explicit.0.clone(),
                extra.0.clone(),
                workspace.0.clone(),
                plugin.0.clone(),
            );
            Ok(SessionSkillCatalogHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionSkillCatalog",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use crate::{
        _base::event::Event,
        app::skill_catalog::{SkillDefinition, SkillMetadata, SkillSource},
    };

    use super::*;

    struct StaticSource {
        id: &'static str,
        priority: i32,
        contribution: RwLock<SkillContribution>,
    }

    #[async_trait]
    impl SkillSourceContract for StaticSource {
        fn id(&self) -> &str {
            self.id
        }
        fn priority(&self) -> i32 {
            self.priority
        }
        fn on_did_change(&self) -> Option<Event<()>> {
            None
        }
        async fn load(&self) -> SkillSourceResult<SkillContribution> {
            Ok(self.contribution.read().clone())
        }
    }

    fn skill(name: &str, description: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.into(),
            description: description.into(),
            path: format!("/{name}/SKILL.md"),
            dir: format!("/{name}"),
            content: description.into(),
            metadata: SkillMetadata::default(),
            source: SkillSource::Project,
            plugin: None,
            mermaid: None,
            d2: None,
        }
    }

    fn source(id: &'static str, priority: i32, skills: Vec<SkillDefinition>) -> Arc<StaticSource> {
        Arc::new(StaticSource {
            id,
            priority,
            contribution: RwLock::new(SkillContribution {
                skills,
                skipped: Some(Vec::new()),
                scanned_roots: Some(Vec::new()),
            }),
        })
    }

    #[tokio::test]
    async fn merges_sources_by_priority_and_supports_sink_overrides() {
        let builtin = source("builtin", 0, vec![skill("base", "builtin")]);
        let user = source("user", 20, vec![skill("collision", "user")]);
        let explicit = source("explicit", 20, vec![skill("collision", "explicit")]);
        let extra = source("extra", 10, vec![skill("extra", "extra")]);
        let workspace = source("workspace", 30, vec![skill("workspace", "workspace")]);
        let plugin = source("plugin", 5, vec![skill("plugin", "plugin")]);
        let service = SessionSkillCatalogService::from_sources(vec![
            builtin, user, explicit, extra, workspace, plugin,
        ]);

        service.ready().await.unwrap();
        assert_eq!(
            service
                .catalog()
                .get_skill("collision")
                .unwrap()
                .description,
            "explicit"
        );
        assert_eq!(service.catalog().list_skills().len(), 5);

        service.set(
            "runtime",
            SkillContribution {
                skills: vec![skill("collision", "runtime")],
                ..SkillContribution::default()
            },
            SkillCatalogSinkOptions { priority: 40 },
        );
        assert_eq!(
            service
                .catalog()
                .get_skill("collision")
                .unwrap()
                .description,
            "runtime"
        );
        service.remove("runtime");
        assert_eq!(
            service
                .catalog()
                .get_skill("collision")
                .unwrap()
                .description,
            "explicit"
        );
    }

    #[tokio::test]
    async fn initial_ready_error_stays_distinct_from_later_reload() {
        struct FailingSource {
            fail: AtomicBool,
        }
        #[async_trait]
        impl SkillSourceContract for FailingSource {
            fn id(&self) -> &str {
                "failing"
            }
            fn priority(&self) -> i32 {
                0
            }
            async fn load(&self) -> SkillSourceResult<SkillContribution> {
                if self.fail.load(Ordering::Relaxed) {
                    return Err(SkillSourceError::Io(io::Error::other("initial failure")));
                }
                Ok(SkillContribution::default())
            }
        }

        let source = Arc::new(FailingSource {
            fail: AtomicBool::new(true),
        });
        let service = SessionSkillCatalogService::from_sources(vec![source.clone()]);
        assert!(service.ready().await.is_err());
        source.fail.store(false, Ordering::Relaxed);
        service.reload().await.unwrap();
        assert!(service.load().await.is_err());
    }
}
