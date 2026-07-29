//! Agent-scoped wire aggregate implementation.
//!
//! Original: `packages/agent-core-v2/src/wire/wireService.ts`.
//!
//! Rust adaptation: model state is protected by a short standard mutex because
//! reducers are synchronous. Blob transforms are serialized through a Tokio
//! task chain, preserving journal order without holding model locks over await.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{Map, Value};
use tokio::task::JoinHandle;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::{
            errors::{BugIndicatingError, Error2Options},
            unexpected_error::on_unexpected_error,
        },
    },
    agent::{
        blob::{AGENT_BLOB_SERVICE_ID, AgentBlobServiceHandle},
        scope_context::AGENT_SCOPE_CONTEXT_ID,
    },
    app::event::event_bus::EVENT_BUS_SERVICE_ID,
    persistence::interface::{
        append_log_store::{
            APPEND_LOG_STORE_SERVICE_ID, AppendLogError, AppendLogOptions, AppendLogStoreHandle,
        },
        storage::{STORAGE_CORRUPTED, StorageError},
    },
};

use super::{
    contract::{CycleError, MAX_DRAIN, WIRE_SERVICE_ID, WireHooks, WireServiceHandle},
    migration::{
        MIGRATE_V1_4_TO_V1_5, MissingWireMigrationError, WIRE_PROTOCOL_VERSION, WireMigration,
        is_newer_wire_version, migrate_wire_record, resolve_wire_migrations,
    },
    model::{ErasedModelDef, ErasedState, ModelDef, PartsTransformer, model_cross_reducers},
    op::{Op, OpTypeError, registered_op},
    record::{
        AGENT_WIRE_RECORD_KEY, WireRecord, create_wire_metadata_record, is_wire_metadata_record,
        is_wire_record, op_to_wire_record, wire_record_to_payload,
    },
};

#[async_trait]
pub trait WireBlobService: Send + Sync {
    async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String>;
    async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String>;
}

pub trait DomainEventPublisher: Send + Sync {
    fn publish(&self, event: Value);
}

impl DomainEventPublisher for crate::app::event::event_bus_service::EventBusService {
    fn publish(&self, event: Value) {
        use crate::app::event::event_bus::{DomainEvent, EventBusContract};

        match DomainEvent::try_from(event) {
            Ok(event) => EventBusContract::publish(self, event),
            Err(error) => on_unexpected_error(&error),
        }
    }
}

impl DomainEventPublisher for crate::app::event::event_bus::EventBusHandle {
    fn publish(&self, event: Value) {
        use crate::app::event::event_bus::DomainEvent;

        match DomainEvent::try_from(event) {
            Ok(event) => self.0.publish(event),
            Err(error) => on_unexpected_error(&error),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestorePhase {
    New,
    Restoring,
    Ready,
    Failed,
}

struct ModelInstance {
    definition: Arc<dyn ErasedModelDef>,
    state: ErasedState,
}

struct RuntimeState {
    models: HashMap<u64, ModelInstance>,
    restore_phase: RestorePhase,
    dispatching: bool,
    queue: VecDeque<Op>,
    drain_depth: usize,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            models: HashMap::new(),
            restore_phase: RestorePhase::New,
            dispatching: false,
            queue: VecDeque::new(),
            drain_depth: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WireServiceError {
    #[error(transparent)]
    Cycle(Box<CycleError>),
    #[error(transparent)]
    AppendLog(#[from] AppendLogError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Migration(#[from] MissingWireMigrationError),
    #[error(transparent)]
    Bug(#[from] BugIndicatingError),
    #[error(transparent)]
    OpType(#[from] OpTypeError),
    #[error("wire model transform failed: {0}")]
    Transform(String),
    #[error("wire restore hook failed: {0}")]
    RestoreHook(String),
    #[error("wire persistence task failed: {0}")]
    PersistenceTask(String),
}

impl From<CycleError> for WireServiceError {
    fn from(error: CycleError) -> Self {
        Self::Cycle(Box::new(error))
    }
}

pub struct WireService {
    hooks: WireHooks,
    scope: String,
    log: AppendLogStoreHandle,
    blob_service: Arc<dyn WireBlobService>,
    event_publisher: Arc<dyn DomainEventPublisher>,
    runtime: Mutex<RuntimeState>,
    persist_tail: Mutex<Option<JoinHandle<()>>>,
    disposables: DisposableStore,
}

impl WireService {
    pub fn new(
        scope: impl Into<String>,
        log: AppendLogStoreHandle,
        blob_service: Arc<dyn WireBlobService>,
        event_publisher: Arc<dyn DomainEventPublisher>,
    ) -> Self {
        let scope = scope.into();
        let disposables = DisposableStore::new();
        disposables.add(log.acquire(&scope, AGENT_WIRE_RECORD_KEY));
        Self {
            hooks: WireHooks::default(),
            scope,
            log,
            blob_service,
            event_publisher,
            runtime: Mutex::new(RuntimeState::default()),
            persist_tail: Mutex::new(None),
            disposables,
        }
    }

    pub fn hooks(&self) -> &WireHooks {
        &self.hooks
    }

    pub fn restore_phase(&self) -> RestorePhase {
        self.runtime.lock().unwrap().restore_phase
    }

    // Original: getModel(). Rust returns a snapshot so the mutex guard cannot
    // escape; wire model states are expected to be cheap immutable values.
    pub fn get_model<S>(&self, model: &ModelDef<S>) -> S
    where
        S: Clone + Send + Sync + 'static,
    {
        self.read_model(model, Clone::clone)
    }

    // Runs a short synchronous read while the model state is locked. Callers
    // must not re-enter WireService from the callback.
    pub(crate) fn read_model<S, R>(&self, model: &ModelDef<S>, read: impl FnOnce(&S) -> R) -> R
    where
        S: Send + Sync + 'static,
    {
        let mut runtime = self.runtime.lock().unwrap();
        let instance = ensure_model(&mut runtime.models, model.erased());
        let state = instance
            .state
            .downcast_ref::<S>()
            .expect("model ID always maps to its defining state type");
        read(state)
    }

    // Original: dispatch(). Reentrant calls enqueue and are drained after the
    // current group; event publication happens outside the model-state lock.
    pub fn dispatch(&self, ops: impl IntoIterator<Item = Op>) -> Result<(), WireServiceError> {
        let ops = ops.into_iter().collect::<Vec<_>>();
        if ops.is_empty() {
            return Ok(());
        }
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.dispatching {
                runtime.queue.extend(ops);
                return Ok(());
            }
            runtime.dispatching = true;
        }

        let result = self.drain(ops);
        let mut runtime = self.runtime.lock().unwrap();
        runtime.queue.clear();
        runtime.dispatching = false;
        runtime.drain_depth = 0;
        result
    }

    fn drain(&self, mut group: Vec<Op>) -> Result<(), WireServiceError> {
        loop {
            self.execute_group(group, false)?;
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.queue.is_empty() {
                return Ok(());
            }
            runtime.drain_depth += 1;
            if runtime.drain_depth > MAX_DRAIN {
                let types = runtime
                    .queue
                    .iter()
                    .map(|op| op.op_type.clone())
                    .collect::<Vec<_>>();
                return Err(CycleError::new(runtime.drain_depth, types).into());
            }
            group = runtime.queue.drain(..).collect();
        }
    }

    fn execute_group(&self, group: Vec<Op>, silent: bool) -> Result<(), WireServiceError> {
        for op in group {
            let model = op.descriptor.model();
            let (event, record) = {
                let mut runtime = self.runtime.lock().unwrap();
                let instance = ensure_model(&mut runtime.models, Arc::clone(&model));
                op.descriptor
                    .validate(instance.state.as_ref(), op.payload())?;
                let previous = std::mem::replace(&mut instance.state, model.initial_state());
                instance.state = op.descriptor.apply(previous, op.payload())?;
                let event = if silent {
                    None
                } else {
                    op.descriptor
                        .to_event(op.payload(), instance.state.as_ref())?
                };
                let record = (!silent && op.descriptor.persist() != Some(false))
                    .then(|| op_to_wire_record(&op));

                for entry in model_cross_reducers(&op.op_type) {
                    if entry.model.id() == model.id() {
                        continue;
                    }
                    let cross = ensure_model(&mut runtime.models, Arc::clone(&entry.model));
                    let previous = std::mem::replace(&mut cross.state, entry.model.initial_state());
                    cross.state = entry
                        .apply(previous, op.payload())
                        .map_err(WireServiceError::Transform)?;
                }
                (event, record)
            };
            if let Some(record) = record {
                self.append_to_journal(record, model);
            }
            if let Some(event) = event {
                self.event_publisher.publish(event);
            }
        }
        Ok(())
    }

    pub async fn seal(&self) -> Result<(), WireServiceError> {
        let mut records = self.log.read::<Value>(&self.scope, AGENT_WIRE_RECORD_KEY);
        if let Some(record) = records.next().await {
            record?;
            return Ok(());
        }
        self.append_record(create_wire_metadata_record().into_wire_record());
        Ok(())
    }

    pub async fn restore(&self) -> Result<(), WireServiceError> {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.restore_phase != RestorePhase::New {
                let message = format!(
                    "Agent wire restore called while phase is {:?}",
                    runtime.restore_phase
                );
                return Err(BugIndicatingError::new(Some(&message)).into());
            }
            runtime.restore_phase = RestorePhase::Restoring;
        }
        let mut result = self.restore_inner().await;
        if result.is_ok() {
            self.runtime.lock().unwrap().restore_phase = RestorePhase::Ready;
            let mut context = ();
            result = self
                .hooks
                .on_did_restore
                .run(&mut context, None)
                .await
                .map_err(|error| WireServiceError::RestoreHook(error.to_string()));
        }
        if result.is_err() {
            self.runtime.lock().unwrap().restore_phase = RestorePhase::Failed;
        }
        result
    }

    async fn restore_inner(&self) -> Result<(), WireServiceError> {
        let mut source = self.log.read::<Value>(&self.scope, AGENT_WIRE_RECORD_KEY);
        let mut migrations: Vec<WireMigration> = Vec::new();
        let mut rewritten: Option<Vec<WireRecord>> = None;
        let mut newer_version = false;
        let mut index = 0usize;
        let mut has_records = false;

        while let Some(candidate) = source.next().await {
            let candidate = candidate?;
            if !is_wire_record(&candidate) {
                report_skipped_record(None, index, true);
                index += 1;
                continue;
            }
            let source_record = candidate.as_object().cloned().expect("validated object");
            if !has_records {
                has_records = true;
                if source_record.get("type").and_then(Value::as_str) != Some("metadata") {
                    rewritten = Some(vec![create_wire_metadata_record().into_wire_record()]);
                    migrations = vec![MIGRATE_V1_4_TO_V1_5];
                } else if !is_wire_metadata_record(&source_record) {
                    return Err(StorageError::new(
                        STORAGE_CORRUPTED,
                        "Agent wire metadata is malformed",
                    )
                    .into());
                } else {
                    let version = source_record["protocol_version"]
                        .as_str()
                        .expect("validated metadata version");
                    if is_newer_wire_version(version) {
                        newer_version = true;
                    } else {
                        migrations = resolve_wire_migrations(version)?;
                        if version != WIRE_PROTOCOL_VERSION {
                            rewritten = Some(Vec::new());
                        }
                    }
                }
            }
            let mut record = migrate_wire_record(&source_record, &migrations);
            if !newer_version && record.get("type").and_then(Value::as_str) == Some("metadata") {
                record.insert(
                    "protocol_version".into(),
                    Value::String(WIRE_PROTOCOL_VERSION.into()),
                );
            }
            if let Some(records) = &mut rewritten {
                records.push(record.clone());
            }
            if record.get("type").and_then(Value::as_str) != Some("metadata") {
                self.replay_record(record, index)?;
                index += 1;
            }
        }
        if !has_records {
            rewritten = Some(vec![create_wire_metadata_record().into_wire_record()]);
        }
        if let Some(records) = rewritten {
            self.log
                .rewrite(&self.scope, AGENT_WIRE_RECORD_KEY, &records)
                .await?;
        }
        self.rehydrate_models().await
    }

    fn replay_record(&self, record: WireRecord, index: usize) -> Result<(), WireServiceError> {
        let op_type = record
            .get("type")
            .and_then(Value::as_str)
            .expect("wire record type was validated");
        let Some(descriptor) = registered_op(op_type) else {
            report_skipped_record(Some(op_type), index, false);
            return Ok(());
        };
        let payload = wire_record_to_payload(&record);
        let op = match Op::from_wire(descriptor, payload) {
            Ok(op) => op,
            Err(_) => {
                report_skipped_record(Some(op_type), index, true);
                return Ok(());
            }
        };
        self.execute_group(vec![op], true)
    }

    fn append_to_journal(&self, record: WireRecord, model: Arc<dyn ErasedModelDef>) {
        let mut tail = self.persist_tail.lock().unwrap();
        if !model.has_blob_codec() && tail.is_none() {
            self.append_record(record);
            return;
        }
        let previous = tail.take();
        let log = self.log.clone();
        let scope = self.scope.clone();
        let blob_service = Arc::clone(&self.blob_service);
        *tail = Some(tokio::spawn(async move {
            if let Some(previous) = previous
                && let Err(error) = previous.await
            {
                on_unexpected_error(&error);
            }
            let transformer = OffloadTransformer(blob_service);
            let record = match model.dehydrate_record(record, &transformer).await {
                Ok(record) => record,
                Err(error) => {
                    on_unexpected_error(&std::io::Error::other(error));
                    return;
                }
            };
            append_record_to(&log, &scope, record);
        }));
    }

    fn append_record(&self, record: WireRecord) {
        append_record_to(&self.log, &self.scope, record);
    }

    async fn rehydrate_models(&self) -> Result<(), WireServiceError> {
        let pending = {
            let mut runtime = self.runtime.lock().unwrap();
            runtime
                .models
                .values_mut()
                .filter(|instance| instance.definition.has_blob_codec())
                .map(|instance| {
                    let state =
                        std::mem::replace(&mut instance.state, instance.definition.initial_state());
                    (
                        instance.definition.id(),
                        Arc::clone(&instance.definition),
                        state,
                    )
                })
                .collect::<Vec<_>>()
        };
        let transformer = LoadTransformer(Arc::clone(&self.blob_service));
        for (id, definition, state) in pending {
            let state = definition
                .rehydrate_state(state, &transformer)
                .await
                .map_err(WireServiceError::Transform)?;
            self.runtime
                .lock()
                .unwrap()
                .models
                .get_mut(&id)
                .expect("rehydrated model remains registered")
                .state = state;
        }
        Ok(())
    }

    pub async fn flush(&self) -> Result<(), WireServiceError> {
        let tail = self.persist_tail.lock().unwrap().take();
        if let Some(tail) = tail {
            tail.await
                .map_err(|error| WireServiceError::PersistenceTask(error.to_string()))?;
        }
        self.log.flush().await?;
        Ok(())
    }
}

impl Disposable for WireService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

impl Drop for WireService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

struct AgentBlobWireAdapter(AgentBlobServiceHandle);

#[async_trait]
impl WireBlobService for AgentBlobWireAdapter {
    async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
        self.0.0.offload_wire_parts(parts).await
    }

    async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
        self.0.0.load_wire_parts(parts).await
    }
}

/// Registers the eager Agent-scoped wire aggregate.
///
/// Original: the module-level `registerScopedService(...)` call in
/// `wireService.ts`.
pub fn register_wire_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        WIRE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let scope_context = accessor.get(AGENT_SCOPE_CONTEXT_ID)?;
            let log = accessor.get(APPEND_LOG_STORE_SERVICE_ID)?;
            let blob_service = accessor.get(AGENT_BLOB_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let wire_blob: Arc<dyn WireBlobService> =
                Arc::new(AgentBlobWireAdapter((*blob_service).clone()));
            let event_publisher: Arc<dyn DomainEventPublisher> = Arc::new((*event_bus).clone());
            Ok(WireServiceHandle(Arc::new(WireService::new(
                scope_context.scope(None),
                (*log).clone(),
                wire_blob,
                event_publisher,
            ))))
        })
        .disposable(),
        InstantiationType::Eager,
        "wire",
    );
}

fn ensure_model(
    models: &mut HashMap<u64, ModelInstance>,
    definition: Arc<dyn ErasedModelDef>,
) -> &mut ModelInstance {
    models
        .entry(definition.id())
        .or_insert_with(|| ModelInstance {
            state: definition.initial_state(),
            definition,
        })
}

fn append_record_to(log: &AppendLogStoreHandle, scope: &str, record: WireRecord) {
    let options = AppendLogOptions {
        on_error: Some(Arc::new(|error| on_unexpected_error(error))),
    };
    if let Err(error) = log.append(scope, AGENT_WIRE_RECORD_KEY, &record, options) {
        on_unexpected_error(&error);
    }
}

fn report_skipped_record(op_type: Option<&str>, index: usize, malformed: bool) {
    use super::errors::{WIRE_UNKNOWN_RECORD, WireError};

    let message = match (op_type, malformed) {
        (None, _) => "Malformed wire record skipped during restore".into(),
        (Some(op_type), true) => {
            format!("Malformed wire record type '{op_type}' skipped during restore")
        }
        (Some(op_type), false) => {
            format!("Unknown wire record type '{op_type}' skipped during restore")
        }
    };
    let details = Map::from_iter([
        (
            "type".into(),
            op_type.map_or(Value::Null, |value| Value::String(value.into())),
        ),
        ("index".into(), Value::from(index as u64)),
    ]);
    let error = WireError::with_options(
        WIRE_UNKNOWN_RECORD,
        message,
        Error2Options {
            details: Some(details),
            ..Error2Options::default()
        },
    );
    on_unexpected_error(&error);
}

struct OffloadTransformer(Arc<dyn WireBlobService>);

#[async_trait]
impl PartsTransformer for OffloadTransformer {
    async fn transform(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
        self.0.offload_parts(parts).await
    }
}

struct LoadTransformer(Arc<dyn WireBlobService>);

#[async_trait]
impl PartsTransformer for LoadTransformer {
    async fn transform(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
        self.0.load_parts(parts).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use futures_util::stream;

    use super::*;
    use crate::{
        _base::di::{
            lifecycle::{Disposable, disposable_none},
            scope::{
                LifecycleScope, Scope, ScopeOptions, clear_scoped_registry_for_tests,
                get_scoped_service_descriptors,
            },
            service_collection::ServiceCollection,
        },
        agent::{
            blob::{AGENT_BLOB_SERVICE_ID, AgentBlobServiceContract, AgentBlobServiceHandle},
            scope_context::{
                AGENT_SCOPE_CONTEXT_ID, AgentScopeContextInput, make_agent_scope_context,
            },
        },
        app::event::{
            event_bus::{EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle},
            event_bus_service::EventBusService,
        },
        kosong::contract::message::ContentPart,
        persistence::interface::append_log_store::{AppendLogStoreService, AppendLogValueStream},
        wire::{
            contract::WIRE_SERVICE_ID,
            model::{ModelOptions, define_model},
            op::DefineOpOptions,
        },
    };

    #[derive(Default)]
    struct MemoryLog {
        records: Mutex<Vec<Value>>,
    }

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, record: Value, _: AppendLogOptions) {
            self.records.lock().unwrap().push(record);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::iter(
                self.records.lock().unwrap().clone().into_iter().map(Ok),
            ))
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            records: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            *self.records.lock().unwrap() = records;
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

    struct IdentityAgentBlobs;

    #[async_trait]
    impl AgentBlobServiceContract for IdentityAgentBlobs {
        async fn offload_parts(
            &self,
            parts: Vec<ContentPart>,
        ) -> Result<Vec<ContentPart>, StorageError> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<ContentPart>) -> Vec<ContentPart> {
            parts
        }

        fn is_blob_ref(&self, _: &str) -> bool {
            false
        }

        async fn offload_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<Value>>);

    impl DomainEventPublisher for Events {
        fn publish(&self, event: Value) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn service(log: Arc<MemoryLog>) -> WireService {
        WireService::new(
            "agents/test",
            AppendLogStoreHandle(log),
            Arc::new(IdentityBlobs),
            Arc::new(Events::default()),
        )
    }

    #[test]
    fn registration_resolves_the_eager_agent_scoped_wire_from_source_dependencies() {
        clear_scoped_registry_for_tests();
        register_wire_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Agent);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == WIRE_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "wire"
        }));

        let mut extra = ServiceCollection::new();
        extra.set_instance(
            AGENT_SCOPE_CONTEXT_ID,
            Arc::new(make_agent_scope_context(AgentScopeContextInput {
                agent_id: "main".into(),
                agent_scope: "sessions/workspace/session/agents/main".into(),
            })),
        );
        let log: Arc<dyn AppendLogStoreService> = Arc::new(MemoryLog::default());
        extra.set_instance(
            APPEND_LOG_STORE_SERVICE_ID,
            Arc::new(AppendLogStoreHandle(log)),
        );
        let blobs: Arc<dyn AgentBlobServiceContract> = Arc::new(IdentityAgentBlobs);
        extra.set_instance(
            AGENT_BLOB_SERVICE_ID,
            Arc::new(AgentBlobServiceHandle(blobs)),
        );
        let events: Arc<dyn EventBusContract> = Arc::new(EventBusService::new());
        extra.set_instance(EVENT_BUS_SERVICE_ID, Arc::new(EventBusHandle(events)));

        let app = Scope::create_app(ScopeOptions::default());
        let session = app
            .create_child(LifecycleScope::Session, "session", ScopeOptions::default())
            .unwrap();
        let agent = session
            .create_child(
                LifecycleScope::Agent,
                "main",
                ScopeOptions { id: None, extra },
            )
            .unwrap();
        let wire = agent.get(WIRE_SERVICE_ID).unwrap();
        assert_eq!(wire.scope, "sessions/workspace/session/agents/main");
        assert_eq!(wire.restore_phase(), RestorePhase::New);

        agent.dispose().unwrap();
        session.dispose().unwrap();
        app.dispose().unwrap();
        clear_scoped_registry_for_tests();
    }

    #[tokio::test]
    async fn dispatch_persists_applies_and_restore_replays_silently() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let model = define_model("counter", || 0_i64, ModelOptions::default());
        let mut options = DefineOpOptions::new(|state, amount: &i64| state + amount);
        options.to_event = Some(Arc::new(|amount, state| {
            Some(serde_json::json!({"amount": amount, "state": state}))
        }));
        let increment = model
            .define_op(format!("wire.test.increment.{suffix}"), options)
            .unwrap();
        let log = Arc::new(MemoryLog::default());
        let first = service(Arc::clone(&log));
        first.dispatch([increment.create(3).unwrap()]).unwrap();
        assert_eq!(first.get_model(&model), 3);
        first.flush().await.unwrap();

        let restored = service(log);
        restored.restore().await.unwrap();
        assert_eq!(restored.restore_phase(), RestorePhase::Ready);
        assert_eq!(restored.get_model(&model), 3);
    }

    #[test]
    fn apply_validation_rejects_before_replacing_live_model_state() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let model = define_model("validated-counter", || 7_i64, ModelOptions::default());
        let op_type = format!(
            "wire.test.validate.{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let op = model
            .define_op(
                op_type,
                DefineOpOptions::new(|state, amount: &i64| state + amount).with_apply_validation(
                    |_state, amount| {
                        if *amount < 0 {
                            Err(Box::new(std::io::Error::other("negative amount")))
                        } else {
                            Ok(())
                        }
                    },
                ),
            )
            .unwrap();
        let service = service(Arc::new(MemoryLog::default()));
        let error = service.dispatch([op.create(-1).unwrap()]).unwrap_err();
        assert!(matches!(
            error,
            WireServiceError::OpType(OpTypeError::Apply { .. })
        ));
        assert_eq!(service.get_model(&model), 7);
    }

    #[tokio::test]
    async fn seal_only_initializes_an_empty_journal() {
        let log = Arc::new(MemoryLog::default());
        let wire = service(Arc::clone(&log));
        wire.seal().await.unwrap();
        wire.seal().await.unwrap();
        let records = log.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["type"], "metadata");
    }
}
