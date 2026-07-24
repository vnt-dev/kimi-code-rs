//! Agent plan-mode state, plan-file persistence, and telemetry restoration.
//!
//! Original: `packages/agent-core-v2/src/agent/plan/planService.ts`.

use std::{
    collections::HashSet,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableHandle, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    _base::utils::hero_slug::generate_hero_slug,
    agent::{
        context_injector::{
            AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceContract,
            AgentContextInjectorServiceHandle,
        },
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContext},
    },
    app::telemetry::{
        AGENT_TELEMETRY_CONTEXT_SERVICE_ID, AgentTelemetryContextPatch,
        AgentTelemetryContextServiceContract, AgentTelemetryContextServiceHandle,
        AgentTelemetryMode,
    },
    hooks::{HookRegisterOptions, HookRegistrationError},
    os::interface::{
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemService, HostFileSystemServiceHandle,
        },
        host_fs_errors::{HostFsError, OS_FS_NOT_FOUND},
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::{WireService, WireServiceError},
    },
};

use super::{
    PLAN_MODE_CANCEL, PLAN_MODE_ENTER, PLAN_MODE_EXIT, PLAN_MODEL, injection::PlanModeInjection,
    plan_mode_cancel, plan_mode_enter, plan_mode_exit,
};

pub type PlanFilePath = Option<String>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanData {
    pub id: String,
    pub content: String,
    pub path: String,
}

#[async_trait]
pub trait AgentPlanServiceContract: Disposable + Send + Sync {
    async fn enter(&self, id: Option<String>, create_file: bool) -> Result<(), PlanServiceError>;
    fn cancel(&self, id: Option<String>) -> Result<(), PlanServiceError>;
    async fn clear(&self) -> Result<(), PlanServiceError>;
    fn exit(&self, id: Option<String>) -> Result<(), PlanServiceError>;
    async fn status(&self) -> Result<Option<PlanData>, PlanServiceError>;
}

#[derive(Clone)]
pub struct AgentPlanServiceHandle(pub Arc<dyn AgentPlanServiceContract>);

impl Deref for AgentPlanServiceHandle {
    type Target = dyn AgentPlanServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentPlanServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_PLAN_SERVICE_ID: ServiceIdentifier<AgentPlanServiceHandle> =
    ServiceIdentifier::new("agentPlanService");

#[derive(Debug, thiserror::Error)]
pub enum PlanServiceError {
    #[error("Already in plan mode")]
    AlreadyActive,
    #[error(transparent)]
    Random(#[from] getrandom::Error),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
    #[error(transparent)]
    HostFs(#[from] HostFsError),
    #[error(transparent)]
    Hook(#[from] HookRegistrationError),
}

pub struct AgentPlanService {
    host_fs: Arc<dyn HostFileSystemService>,
    telemetry_context: Arc<dyn AgentTelemetryContextServiceContract>,
    wire: Arc<WireService>,
    session_context: SessionContext,
    agent_context: AgentScopeContext,
    disposables: DisposableStore,
}

impl AgentPlanService {
    // Original: planService.ts, AgentPlanService.constructor(). Construction
    // stays fallible because Rust hook registration reports placement errors.
    pub fn new(
        context: Arc<dyn AgentContextMemoryServiceContract>,
        host_fs: Arc<dyn HostFileSystemService>,
        injector: Arc<dyn AgentContextInjectorServiceContract>,
        telemetry_context: Arc<dyn AgentTelemetryContextServiceContract>,
        wire: Arc<WireService>,
        session_context: SessionContext,
        agent_context: AgentScopeContext,
    ) -> Result<Arc<Self>, PlanServiceError> {
        // The source module import registers the model and every replayable
        // operation before Wire restoration. Force the equivalent Rust lazies
        // eagerly for persisted `plan_mode.*` records.
        std::sync::LazyLock::force(&PLAN_MODEL);
        std::sync::LazyLock::force(&PLAN_MODE_ENTER);
        std::sync::LazyLock::force(&PLAN_MODE_CANCEL);
        std::sync::LazyLock::force(&PLAN_MODE_EXIT);
        let service = Arc::new(Self {
            host_fs,
            telemetry_context,
            wire,
            session_context,
            agent_context,
            disposables: DisposableStore::new(),
        });
        service.install_restore_hook()?;
        let plan: Arc<dyn AgentPlanServiceContract> = service.clone();
        let injection: DisposableHandle = Arc::new(PlanModeInjection::new(injector, plan, context));
        service.disposables.add(injection);
        Ok(service)
    }

    // Original: planService.ts, dependency-injected constructor.
    pub fn from_handles(
        context: AgentContextMemoryServiceHandle,
        host_fs: HostFileSystemServiceHandle,
        injector: AgentContextInjectorServiceHandle,
        telemetry_context: AgentTelemetryContextServiceHandle,
        wire: WireServiceHandle,
        session_context: SessionContext,
        agent_context: AgentScopeContext,
    ) -> Result<Arc<Self>, PlanServiceError> {
        Self::new(
            context.0,
            host_fs.0,
            injector.0,
            telemetry_context.0,
            wire.0,
            session_context,
            agent_context,
        )
    }

    fn install_restore_hook(self: &Arc<Self>) -> Result<(), PlanServiceError> {
        let weak = Arc::downgrade(self);
        let hook = self.wire.hooks().on_did_restore.register(
            "plan",
            Arc::new(move |context, next| {
                let weak = weak.clone();
                Box::pin(async move {
                    if let Some(service) = weak.upgrade() {
                        service.restore_telemetry_mode();
                    }
                    next(context).await
                })
            }),
            HookRegisterOptions::default(),
        )?;
        self.disposables.add(hook);
        Ok(())
    }

    // Original: AgentPlanService.isActive getter.
    fn is_active(&self) -> bool {
        self.wire.get_model(&PLAN_MODEL).active
    }

    // Original: AgentPlanService.currentPlanFilePath().
    fn current_plan_file_path(&self) -> PlanFilePath {
        let state = self.wire.get_model(&PLAN_MODEL);
        (state.active)
            .then(|| state.id)
            .flatten()
            .map(|id| self.plan_file_path_for(&id))
    }

    // Original: AgentPlanService.restoreTelemetryMode().
    fn restore_telemetry_mode(&self) {
        if self.is_active() {
            self.telemetry_context.set(AgentTelemetryContextPatch {
                mode: Some(AgentTelemetryMode::Plan),
                ..AgentTelemetryContextPatch::default()
            });
        }
    }

    // Original: AgentPlanService.createPlanId(). `randomUUID()` is represented
    // by a v4 UUID before applying the same hero-slug generator.
    fn create_plan_id(&self) -> Result<String, getrandom::Error> {
        generate_hero_slug(&Uuid::new_v4().to_string(), &HashSet::new())
    }

    // Original: AgentPlanService.planFilePathFor().
    fn plan_file_path_for(&self, id: &str) -> String {
        PathBuf::from(&self.session_context.session_dir)
            .join("agents")
            .join(&self.agent_context.agent_id)
            .join("plans")
            .join(format!("{id}.md"))
            .to_string_lossy()
            .into_owned()
    }

    // Original: AgentPlanService.writeEmptyPlanFile().
    async fn write_empty_plan_file(&self, path: &str) -> Result<(), HostFsError> {
        self.ensure_plan_directory(path).await?;
        self.host_fs.write_text(Path::new(path), "").await
    }

    // Original: AgentPlanService.ensurePlanDirectory().
    async fn ensure_plan_directory(&self, path: &str) -> Result<(), HostFsError> {
        if let Some(directory) = Path::new(path).parent() {
            self.host_fs.create_dir(directory, true).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl AgentPlanServiceContract for AgentPlanService {
    // Original: AgentPlanService.enter(). The source's catch path only
    // compensates after the enter operation has been recorded; preserve that
    // ordering and intentionally ignore a cancellation failure.
    async fn enter(&self, id: Option<String>, create_file: bool) -> Result<(), PlanServiceError> {
        if self.is_active() {
            return Err(PlanServiceError::AlreadyActive);
        }
        let id = match id {
            Some(id) => id,
            None => self.create_plan_id()?,
        };
        let path = self.plan_file_path_for(&id);
        let mut enter_recorded = false;
        let result = async {
            self.ensure_plan_directory(&path).await?;
            self.wire.dispatch([plan_mode_enter(id.clone())?])?;
            self.telemetry_context.set(AgentTelemetryContextPatch {
                mode: Some(AgentTelemetryMode::Plan),
                ..AgentTelemetryContextPatch::default()
            });
            enter_recorded = true;
            if create_file {
                self.write_empty_plan_file(&path).await?;
            }
            Ok(())
        }
        .await;
        if result.is_err() && enter_recorded {
            let _ = self.cancel(Some(id));
        }
        result
    }

    // Original: AgentPlanService.cancel().
    fn cancel(&self, id: Option<String>) -> Result<(), PlanServiceError> {
        self.wire.dispatch([plan_mode_cancel(id)?])?;
        self.telemetry_context.set(AgentTelemetryContextPatch {
            mode: Some(AgentTelemetryMode::Agent),
            ..AgentTelemetryContextPatch::default()
        });
        Ok(())
    }

    // Original: AgentPlanService.clear().
    async fn clear(&self) -> Result<(), PlanServiceError> {
        let Some(path) = self.current_plan_file_path() else {
            return Ok(());
        };
        self.write_empty_plan_file(&path).await?;
        Ok(())
    }

    // Original: AgentPlanService.exit().
    fn exit(&self, id: Option<String>) -> Result<(), PlanServiceError> {
        self.wire.dispatch([plan_mode_exit(id)?])?;
        self.telemetry_context.set(AgentTelemetryContextPatch {
            mode: Some(AgentTelemetryMode::Agent),
            ..AgentTelemetryContextPatch::default()
        });
        Ok(())
    }

    // Original: AgentPlanService.status(). Missing plan files intentionally
    // report empty content while all other filesystem errors propagate.
    async fn status(&self) -> Result<Option<PlanData>, PlanServiceError> {
        let state = self.wire.get_model(&PLAN_MODEL);
        if !state.active {
            return Ok(None);
        }
        let Some(id) = state.id else {
            return Ok(None);
        };
        let path = self.plan_file_path_for(&id);
        let content = match self.host_fs.read_text(Path::new(&path), None).await {
            Ok(content) => content,
            Err(error) if error.code() == OS_FS_NOT_FOUND => String::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(PlanData { id, content, path }))
    }
}

impl Disposable for AgentPlanService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

impl Drop for AgentPlanService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

// Original: planService.ts, registerScopedService(..., Eager, "plan").
pub fn register_agent_plan_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PLAN_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let host_fs = accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?;
            let injector = accessor.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?;
            let telemetry_context = accessor.get(AGENT_TELEMETRY_CONTEXT_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let session_context = accessor.get(SESSION_CONTEXT_ID)?;
            let agent_context = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let service = AgentPlanService::from_handles(
                (*context).clone(),
                (*host_fs).clone(),
                (*injector).clone(),
                (*telemetry_context).clone(),
                (*wire).clone(),
                (*session_context).clone(),
                (*agent_context).clone(),
            )
            .map_err(|error| crate::_base::di::errors::DiError::Factory(error.to_string()))?;
            let service: Arc<dyn AgentPlanServiceContract> = service;
            Ok(AgentPlanServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "plan",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::di::lifecycle::disposable_none,
        agent::{
            context_injector::{ContextInjectionError, ContextInjectionProvider},
            context_memory::{
                ContextCompactionInput, ContextCompactionResult, ContextMemoryServiceError,
                loop_event_fold::LoopRecordedEvent, undo::UndoCut,
            },
            scope_context::{AgentScopeContextInput, make_agent_scope_context},
        },
        app::telemetry::AgentTelemetryContextService,
        os::{
            backends::node_local::host_fs_service::HostFileSystem,
            interface::host_file_system::HostFileSystemService,
        },
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        session::session_context::{SessionContextInput, make_session_context},
        wire::wire_service::{DomainEventPublisher, WireBlobService},
    };

    #[derive(Default)]
    struct MemoryLog(Mutex<Vec<Value>>);

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, value: Value, _: AppendLogOptions) {
            self.0.lock().unwrap().push(value);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::empty())
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            _: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn flush(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        async fn close(&self) -> Result<(), AppendLogError> {
            Ok(())
        }

        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    struct IdentityBlobs;

    #[async_trait]
    impl WireBlobService for IdentityBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct NoopEvents;

    impl DomainEventPublisher for NoopEvents {
        fn publish(&self, _: Value) {}
    }

    struct EmptyContext;

    impl AgentContextMemoryServiceContract for EmptyContext {
        fn get(&self) -> Vec<crate::agent::context_memory::ContextMessage> {
            Vec::new()
        }

        fn append(
            &self,
            _: Vec<crate::agent::context_memory::ContextMessage>,
        ) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn append_loop_event(&self, _: LoopRecordedEvent) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn undo(&self, _: f64) -> Result<UndoCut, ContextMemoryServiceError> {
            unreachable!("plan injection only reads context history")
        }

        fn apply_compaction(
            &self,
            _: ContextCompactionInput,
        ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
            unreachable!("plan injection only reads context history")
        }
    }

    struct NoopInjector;

    impl Disposable for NoopInjector {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[async_trait]
    impl AgentContextInjectorServiceContract for NoopInjector {
        fn register(&self, _: String, _: ContextInjectionProvider) -> DisposableHandle {
            disposable_none()
        }

        async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
            Ok(())
        }
    }

    fn setup() -> (
        std::path::PathBuf,
        Arc<dyn HostFileSystemService>,
        Arc<AgentTelemetryContextService>,
        Arc<WireService>,
        Arc<AgentPlanService>,
    ) {
        let root = std::env::temp_dir().join(format!("kimi-plan-service-test-{}", Uuid::new_v4()));
        let log = Arc::new(MemoryLog::default());
        let wire = Arc::new(WireService::new(
            "agents/plan-test",
            AppendLogStoreHandle(log),
            Arc::new(IdentityBlobs),
            Arc::new(NoopEvents),
        ));
        let host_fs: Arc<dyn HostFileSystemService> = Arc::new(HostFileSystem);
        let telemetry = Arc::new(AgentTelemetryContextService::new());
        let session = make_session_context(SessionContextInput {
            session_id: "session".into(),
            workspace_id: "workspace".into(),
            session_dir: root.to_string_lossy().into_owned(),
            session_scope: "sessions/workspace/session".into(),
            cwd: "/workspace".into(),
            meta_scope: None,
        });
        let agent = make_agent_scope_context(AgentScopeContextInput {
            agent_id: "main".into(),
            agent_scope: "sessions/workspace/session/agents/main".into(),
        });
        let service = AgentPlanService::new(
            Arc::new(EmptyContext),
            Arc::clone(&host_fs),
            Arc::new(NoopInjector),
            telemetry.clone(),
            wire.clone(),
            session,
            agent,
        )
        .unwrap();
        (root, host_fs, telemetry, wire, service)
    }

    #[tokio::test]
    async fn enter_status_clear_and_exit_preserve_plan_file_and_telemetry_behavior() {
        let (root, host_fs, telemetry, wire, service) = setup();
        service.enter(Some("plan-1".into()), true).await.unwrap();
        assert_eq!(
            wire.get_model(&PLAN_MODEL),
            crate::agent::plan::PlanState {
                active: true,
                id: Some("plan-1".into()),
            }
        );
        assert_eq!(telemetry.get().mode, AgentTelemetryMode::Plan);

        let status = service.status().await.unwrap().unwrap();
        assert!(status.content.is_empty());
        assert!(status.path.ends_with("agents/main/plans/plan-1.md"));
        host_fs
            .write_text(Path::new(&status.path), "draft")
            .await
            .unwrap();
        assert_eq!(service.status().await.unwrap().unwrap().content, "draft");
        service.clear().await.unwrap();
        assert_eq!(service.status().await.unwrap().unwrap().content, "");

        service.exit(Some("ignored".into())).unwrap();
        assert_eq!(wire.get_model(&PLAN_MODEL).active, false);
        assert_eq!(telemetry.get().mode, AgentTelemetryMode::Agent);
        assert_eq!(service.status().await.unwrap(), None);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn status_treats_a_missing_plan_file_as_empty_content() {
        let (root, _, _, _, service) = setup();
        service.enter(Some("missing".into()), false).await.unwrap();
        let status = service.status().await.unwrap().unwrap();
        assert_eq!(status.content, "");
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
