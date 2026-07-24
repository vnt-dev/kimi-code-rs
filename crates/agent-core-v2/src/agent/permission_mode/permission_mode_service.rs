use std::{ops::Deref, sync::Arc};

use crate::{
    _base::{
        di::{
            instantiation::ServiceIdentifier,
            lifecycle::{DisposableHandle, DisposableStore},
        },
        event::{Emitter, Event},
    },
    agent::{
        context_injector::AgentContextInjectorServiceContract, permission_policy::PermissionMode,
    },
    wire::wire_service::{WireService, WireServiceError},
};

use super::{
    injection::{PermissionModeInjection, PermissionModeReader},
    permission_mode_ops::{
        PERMISSION_MODE_CONFIGURED_MODEL, PERMISSION_MODE_MODEL, SET_PERMISSION_MODE,
        set_permission_mode,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionModeChangedContext {
    pub mode: PermissionMode,
    pub previous_mode: PermissionMode,
}

pub trait AgentPermissionModeServiceContract: Send + Sync {
    fn mode(&self) -> PermissionMode;
    fn set_mode(&self, mode: PermissionMode) -> Result<(), PermissionModeServiceError>;
    fn on_did_change_mode(&self) -> Event<PermissionModeChangedContext>;
}

#[derive(Clone)]
pub struct AgentPermissionModeServiceHandle(pub Arc<dyn AgentPermissionModeServiceContract>);

impl Deref for AgentPermissionModeServiceHandle {
    type Target = dyn AgentPermissionModeServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_PERMISSION_MODE_SERVICE_ID: ServiceIdentifier<AgentPermissionModeServiceHandle> =
    ServiceIdentifier::new("agentPermissionModeService");

#[derive(Debug, thiserror::Error)]
pub enum PermissionModeServiceError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
}

struct WirePermissionModeReader {
    wire: Arc<WireService>,
}

impl PermissionModeReader for WirePermissionModeReader {
    fn mode(&self) -> PermissionMode {
        self.wire.get_model(&PERMISSION_MODE_MODEL)
    }
}

pub struct AgentPermissionModeService {
    wire: Arc<WireService>,
    changed: Arc<Emitter<PermissionModeChangedContext>>,
    _disposables: DisposableStore,
}

impl AgentPermissionModeService {
    // Original: permissionModeService.ts, AgentPermissionModeService.constructor().
    pub fn new(wire: Arc<WireService>, injector: &dyn AgentContextInjectorServiceContract) -> Self {
        std::sync::LazyLock::force(&PERMISSION_MODE_CONFIGURED_MODEL);
        std::sync::LazyLock::force(&SET_PERMISSION_MODE);
        let changed = Arc::new(Emitter::new());
        let injection = Arc::new(PermissionModeInjection::new(
            Arc::new(WirePermissionModeReader {
                wire: Arc::clone(&wire),
            }),
            injector,
        ));
        let disposables = DisposableStore::new();
        let changed_disposable: DisposableHandle = changed.clone();
        disposables.add(changed_disposable);
        let injection_disposable: DisposableHandle = injection;
        disposables.add(injection_disposable);
        Self {
            wire,
            changed,
            _disposables: disposables,
        }
    }
}

impl AgentPermissionModeServiceContract for AgentPermissionModeService {
    // Original: AgentPermissionModeService.mode getter.
    fn mode(&self) -> PermissionMode {
        self.wire.get_model(&PERMISSION_MODE_MODEL)
    }

    // Original: AgentPermissionModeService.setMode().
    fn set_mode(&self, mode: PermissionMode) -> Result<(), PermissionModeServiceError> {
        let previous_mode = self.mode();
        let changed = mode != previous_mode;
        if !changed && self.wire.get_model(&PERMISSION_MODE_CONFIGURED_MODEL) {
            return Ok(());
        }
        self.wire.dispatch([set_permission_mode(mode)?])?;
        if changed {
            self.changed.fire(&PermissionModeChangedContext {
                mode,
                previous_mode,
            });
        }
        Ok(())
    }

    fn on_did_change_mode(&self) -> Event<PermissionModeChangedContext> {
        self.changed.event()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::di::lifecycle::{disposable_none, to_disposable},
        agent::context_injector::{ContextInjectionError, ContextInjectionProvider},
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::wire_service::{DomainEventPublisher, WireBlobService},
    };

    #[derive(Default)]
    struct CountingLog {
        appended: AtomicUsize,
    }

    #[async_trait]
    impl AppendLogStoreService for CountingLog {
        fn append_value(&self, _: &str, _: &str, _: Value, _: AppendLogOptions) {
            self.appended.fetch_add(1, Ordering::SeqCst);
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
        fn acquire(&self, _: &str, _: &str) -> crate::_base::di::lifecycle::DisposableHandle {
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

    #[derive(Default)]
    struct Injector {
        provider: Mutex<Option<ContextInjectionProvider>>,
    }

    #[async_trait]
    impl AgentContextInjectorServiceContract for Injector {
        fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle {
            assert_eq!(name, "permission_mode");
            *self.provider.lock().unwrap() = Some(provider);
            to_disposable(|| {})
        }

        async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
            Ok(())
        }
    }

    fn setup() -> (
        Arc<WireService>,
        Arc<CountingLog>,
        Injector,
        AgentPermissionModeService,
    ) {
        let log = Arc::new(CountingLog::default());
        let log_handle: Arc<dyn AppendLogStoreService> = log.clone();
        let wire = Arc::new(WireService::new(
            "agents/permission-mode-test",
            AppendLogStoreHandle(log_handle),
            Arc::new(IdentityBlobs),
            Arc::new(NoopEvents),
        ));
        let injector = Injector::default();
        let service = AgentPermissionModeService::new(Arc::clone(&wire), &injector);
        (wire, log, injector, service)
    }

    #[tokio::test]
    async fn explicit_initial_mode_persists_once_and_changes_emit_only_on_difference() {
        let (wire, log, injector, service) = setup();
        assert!(injector.provider.lock().unwrap().is_some());
        let changes = Arc::new(Mutex::new(Vec::new()));
        let changes_for_listener = Arc::clone(&changes);
        let _subscription = service.on_did_change_mode().subscribe(move |change| {
            changes_for_listener.lock().unwrap().push(*change);
        });

        assert_eq!(service.mode(), PermissionMode::Manual);
        service.set_mode(PermissionMode::Manual).unwrap();
        assert!(wire.get_model(&PERMISSION_MODE_CONFIGURED_MODEL));
        service.set_mode(PermissionMode::Manual).unwrap();
        service.set_mode(PermissionMode::Auto).unwrap();
        service.set_mode(PermissionMode::Auto).unwrap();
        assert_eq!(service.mode(), PermissionMode::Auto);
        assert_eq!(
            *changes.lock().unwrap(),
            [PermissionModeChangedContext {
                mode: PermissionMode::Auto,
                previous_mode: PermissionMode::Manual,
            }]
        );
        wire.flush().await.unwrap();
        assert_eq!(log.appended.load(Ordering::SeqCst), 2);
    }
}
