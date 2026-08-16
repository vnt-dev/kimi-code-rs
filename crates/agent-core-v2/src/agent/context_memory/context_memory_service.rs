use std::{
    ops::Deref,
    sync::{Arc, LazyLock},
};

use serde_json::{Map, Value};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        context_size::{
            CONTEXT_SIZE_MEASURED, CONTEXT_SIZE_MODEL, ContextSizeMeasuredPayload,
            context_size_measured,
        },
        swarm::SWARM_EXIT,
    },
    app::event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle},
    kosong::contract::tokens::estimate_tokens_for_messages,
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        op::Op,
        wire_service::{WireService, WireServiceError},
    },
};

use super::{
    compaction_handoff::{ContextCompactionShapeInput, build_context_compaction_shape},
    context_ops::{
        CONTEXT_APPEND_LOOP_EVENT, CONTEXT_APPEND_MESSAGE, CONTEXT_APPLY_COMPACTION, CONTEXT_CLEAR,
        CONTEXT_MODEL, CONTEXT_UNDO, context_append_loop_event, context_append_message,
        context_apply_compaction, context_clear, context_undo,
    },
    loop_event_fold::LoopRecordedEvent,
    types::ContextMessage,
    undo::{UndoCut, compute_undo_cut, is_fully_undoable},
};

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionInput {
    pub summary: String,
    pub context_summary: Option<String>,
    pub compacted_count: u64,
    pub tokens_before: u64,
    pub tokens_after: Option<u64>,
    pub kept_user_message_count: Option<u64>,
    pub kept_head_user_message_count: Option<u64>,
    pub dropped_count: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextCompactionResult {
    pub summary: String,
    pub context_summary: String,
    pub compacted_count: u64,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub kept_user_message_count: u64,
    pub kept_head_user_message_count: Option<u64>,
    pub dropped_count: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContextMemorySnapshot(Arc<Vec<ContextMessage>>);

impl ContextMemorySnapshot {
    fn from_shared(messages: Arc<Vec<ContextMessage>>) -> Self {
        Self(messages)
    }
}

impl From<Vec<ContextMessage>> for ContextMemorySnapshot {
    fn from(messages: Vec<ContextMessage>) -> Self {
        Self(Arc::new(messages))
    }
}

impl FromIterator<ContextMessage> for ContextMemorySnapshot {
    fn from_iter<T: IntoIterator<Item = ContextMessage>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl Deref for ContextMemorySnapshot {
    type Target = [ContextMessage];

    fn deref(&self) -> &Self::Target {
        self.0.as_slice()
    }
}

impl AsRef<[ContextMessage]> for ContextMemorySnapshot {
    fn as_ref(&self) -> &[ContextMessage] {
        self
    }
}

pub trait AgentContextMemoryServiceContract: Send + Sync {
    fn get(&self) -> ContextMemorySnapshot;
    fn append(&self, messages: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError>;
    fn append_loop_event(&self, event: LoopRecordedEvent) -> Result<(), ContextMemoryServiceError>;
    fn clear(&self) -> Result<(), ContextMemoryServiceError>;
    fn undo(&self, count: u32) -> Result<UndoCut, ContextMemoryServiceError>;
    fn apply_compaction(
        &self,
        input: ContextCompactionInput,
    ) -> Result<ContextCompactionResult, ContextMemoryServiceError>;
}

#[derive(Clone)]
pub struct AgentContextMemoryServiceHandle(pub Arc<dyn AgentContextMemoryServiceContract>);

impl Deref for AgentContextMemoryServiceHandle {
    type Target = dyn AgentContextMemoryServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_CONTEXT_MEMORY_SERVICE_ID: ServiceIdentifier<AgentContextMemoryServiceHandle> =
    ServiceIdentifier::new("agentContextMemoryService");

#[derive(Debug, thiserror::Error)]
pub enum ContextMemoryServiceError {
    #[error(transparent)]
    Wire(#[from] WireServiceError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

pub struct AgentContextMemoryService {
    wire: Arc<WireService>,
    event_bus: Arc<dyn EventBusContract>,
}

impl AgentContextMemoryService {
    // Original: contextMemoryService.ts, constructor(). Forcing every lazy Op
    // here guarantees replay registration before WireService.restore().
    pub fn new(wire: Arc<WireService>, event_bus: Arc<dyn EventBusContract>) -> Self {
        LazyLock::force(&CONTEXT_APPEND_MESSAGE);
        LazyLock::force(&CONTEXT_APPEND_LOOP_EVENT);
        LazyLock::force(&CONTEXT_CLEAR);
        LazyLock::force(&CONTEXT_APPLY_COMPACTION);
        LazyLock::force(&CONTEXT_UNDO);
        LazyLock::force(&CONTEXT_SIZE_MEASURED);
        LazyLock::force(&SWARM_EXIT);
        Self { wire, event_bus }
    }

    // Original: contextMemoryService.ts, dependency-injected constructor.
    pub fn from_handles(wire: WireServiceHandle, event_bus: EventBusHandle) -> Self {
        Self::new(wire.0, event_bus.0)
    }

    fn publish_splice(
        &self,
        start: usize,
        delete_count: usize,
        messages: &[ContextMessage],
        tokens: Option<u64>,
    ) -> Result<(), serde_json::Error> {
        let messages = serde_json::to_value(messages)?;
        self.publish_splice_value(start, delete_count, messages, tokens)
    }

    /// Publishes a `context.spliced` event from a pre-serialized message list,
    /// avoiding a clone of the messages when they are consumed elsewhere.
    fn publish_splice_value(
        &self,
        start: usize,
        delete_count: usize,
        messages: Value,
        tokens: Option<u64>,
    ) -> Result<(), serde_json::Error> {
        let mut fields = Map::from_iter([
            ("start".into(), Value::from(start as u64)),
            ("deleteCount".into(), Value::from(delete_count as u64)),
            ("messages".into(), messages),
        ]);
        if let Some(tokens) = tokens {
            fields.insert("tokens".into(), serde_json::to_value(tokens)?);
        }
        self.event_bus
            .publish(DomainEvent::new("context.spliced", fields));
        Ok(())
    }

    fn size_ops_for_cut(
        &self,
        cut_index: usize,
        history: &[ContextMessage],
    ) -> Result<Vec<Op>, serde_json::Error> {
        let model = self.wire.get_model(&CONTEXT_SIZE_MODEL);
        if model.length <= cut_index as u64 {
            return Ok(Vec::new());
        }
        Ok(vec![context_size_measured(ContextSizeMeasuredPayload {
            length: cut_index as u64,
            tokens: estimate_tokens_for_messages(
                history[..cut_index].iter().map(|message| &message.message),
            ) as u64,
        })?])
    }
}

// Original: contextMemoryService.ts, registerScopedService(..., Eager,
// "contextMemory").
pub fn register_agent_context_memory_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_CONTEXT_MEMORY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let service: Arc<dyn AgentContextMemoryServiceContract> = Arc::new(
                AgentContextMemoryService::from_handles((*wire).clone(), (*event_bus).clone()),
            );
            Ok(AgentContextMemoryServiceHandle(service))
        }),
        InstantiationType::Eager,
        "contextMemory",
    );
}

impl AgentContextMemoryServiceContract for AgentContextMemoryService {
    // Original: contextMemoryService.ts, get(). The snapshot shares immutable
    // storage with the model and remains stable through copy-on-write updates.
    fn get(&self) -> ContextMemorySnapshot {
        ContextMemorySnapshot::from_shared(
            self.wire
                .read_model(&CONTEXT_MODEL, |state| state.messages_snapshot()),
        )
    }

    // Original: contextMemoryService.ts, append().
    fn append(&self, messages: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError> {
        if messages.is_empty() {
            return Ok(());
        }
        let start = self.get().len();
        let splice_messages = serde_json::to_value(&messages)?;
        let ops = messages
            .into_iter()
            .map(context_append_message)
            .collect::<Result<Vec<_>, _>>()?;
        self.wire.dispatch(ops)?;
        self.publish_splice_value(start, 0, splice_messages, None)?;
        Ok(())
    }

    fn append_loop_event(&self, event: LoopRecordedEvent) -> Result<(), ContextMemoryServiceError> {
        self.wire.dispatch([context_append_loop_event(event)?])?;
        Ok(())
    }

    // Original: contextMemoryService.ts, clear().
    fn clear(&self) -> Result<(), ContextMemoryServiceError> {
        let delete_count = self.get().len();
        if delete_count == 0 {
            return Ok(());
        }
        self.wire.dispatch([
            context_clear()?,
            context_size_measured(ContextSizeMeasuredPayload {
                length: 0,
                tokens: 0,
            })?,
        ])?;
        self.publish_splice(0, delete_count, &[], None)?;
        Ok(())
    }

    // Original: contextMemoryService.ts, undo().
    fn undo(&self, count: u32) -> Result<UndoCut, ContextMemoryServiceError> {
        let history = self.get();
        let cut = compute_undo_cut(&history, count);
        if is_fully_undoable(cut, count) {
            let cut_index = usize::try_from(cut.cut_index).unwrap_or(0);
            let mut ops = vec![context_undo(count)?];
            ops.extend(self.size_ops_for_cut(cut_index, &history)?);
            self.wire.dispatch(ops)?;
            self.publish_splice(cut_index, history.len() - cut_index, &[], None)?;
        }
        Ok(cut)
    }

    // Original: contextMemoryService.ts, applyCompaction().
    fn apply_compaction(
        &self,
        input: ContextCompactionInput,
    ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
        let history = self.get();
        let shape_input = ContextCompactionShapeInput {
            summary: input.summary,
            legacy_summary_message: None,
            context_summary: input.context_summary,
            compacted_count: input.compacted_count,
            tokens_before: input.tokens_before,
            tokens_after: input.tokens_after,
            kept_user_message_count: input.kept_user_message_count,
            kept_head_user_message_count: input.kept_head_user_message_count,
            dropped_count: input.dropped_count,
            legacy_tail: None,
        };
        let result = build_context_compaction_shape(&history, shape_input);
        let persisted_input = ContextCompactionShapeInput {
            summary: result.summary.clone(),
            legacy_summary_message: None,
            context_summary: Some(result.context_summary.clone()),
            compacted_count: result.compacted_count,
            tokens_before: result.tokens_before,
            tokens_after: Some(result.tokens_after),
            kept_user_message_count: Some(result.kept_user_message_count),
            kept_head_user_message_count: result.kept_head_user_message_count,
            dropped_count: result.dropped_count,
            legacy_tail: None,
        };
        self.wire.dispatch([
            context_apply_compaction(persisted_input)?,
            context_size_measured(ContextSizeMeasuredPayload {
                length: result.messages.len() as u64,
                tokens: result.tokens_after,
            })?,
        ])?;
        self.publish_splice(
            0,
            history.len(),
            &result.messages,
            Some(result.tokens_after),
        )?;
        Ok(ContextCompactionResult {
            summary: result.summary,
            context_summary: result.context_summary,
            compacted_count: result.compacted_count,
            tokens_before: result.tokens_before,
            tokens_after: result.tokens_after,
            kept_user_message_count: result.kept_user_message_count,
            kept_head_user_message_count: result.kept_head_user_message_count,
            dropped_count: result.dropped_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;

    use async_trait::async_trait;
    use futures_util::stream;

    use super::*;
    use crate::{
        _base::di::lifecycle::disposable_none,
        app::event::event_bus_service::EventBusService,
        kosong::contract::message::{ContentPart, Message, Role},
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::wire_service::{DomainEventPublisher, WireBlobService},
    };

    #[derive(Default)]
    struct MemoryLog(Mutex<Vec<Value>>);

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, value: Value, _: AppendLogOptions) {
            self.0.lock().push(value);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::iter(self.0.lock().clone().into_iter().map(Ok)))
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            values: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            *self.0.lock() = values;
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

    fn user(text: &str) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text { text: text.into() }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: Some(super::super::types::PromptOrigin::User),
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    fn service() -> (AgentContextMemoryService, Arc<EventBusService>) {
        let events = Arc::new(EventBusService::new());
        let publisher: Arc<dyn DomainEventPublisher> = events.clone();
        let wire = Arc::new(WireService::new(
            "agents/test",
            AppendLogStoreHandle(Arc::new(MemoryLog::default())),
            Arc::new(IdentityBlobs),
            publisher,
        ));
        let bus: Arc<dyn EventBusContract> = events.clone();
        (AgentContextMemoryService::new(wire, bus), events)
    }

    #[tokio::test]
    async fn append_undo_compact_and_clear_update_model_and_publish_splices() {
        let (service, events) = service();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let target = Arc::clone(&seen);
        let _subscription = events.subscribe_type(
            "context.spliced",
            Arc::new(move |event| target.lock().push(event.clone())),
        );

        service.append(vec![user("one"), user("two")]).unwrap();
        assert_eq!(service.get().len(), 2);
        let cut = service.undo(1).unwrap();
        assert_eq!(cut.cut_index, 1);
        assert_eq!(service.get().len(), 1);

        let result = service
            .apply_compaction(ContextCompactionInput {
                summary: "summary".into(),
                context_summary: None,
                compacted_count: 1,
                tokens_before: 3,
                tokens_after: Some(2),
                kept_user_message_count: None,
                kept_head_user_message_count: None,
                dropped_count: None,
            })
            .unwrap();
        assert_eq!(result.tokens_after, 2);
        assert_eq!(service.wire.get_model(&CONTEXT_SIZE_MODEL).tokens, 2);
        service.clear().unwrap();
        assert!(service.get().is_empty());
        assert_eq!(seen.lock().len(), 4);
        service.wire.flush().await.unwrap();
    }

    #[tokio::test]
    async fn empty_mutations_are_noops_and_existing_ids_are_preserved() {
        let (service, _) = service();
        service.append(Vec::new()).unwrap();
        service.clear().unwrap();
        let mut message = user("id");
        message.id = Some("msg_existing".into());
        service.append(vec![message]).unwrap();
        assert_eq!(service.get()[0].id.as_deref(), Some("msg_existing"));
        service.wire.flush().await.unwrap();
    }

    #[tokio::test]
    async fn context_snapshots_share_storage_and_remain_stable_after_writes() {
        let (service, _) = service();
        service.append(vec![user("one")]).unwrap();

        let first = service.get();
        let same_history = service.get();
        assert!(Arc::ptr_eq(&first.0, &same_history.0));

        service.append(vec![user("two")]).unwrap();
        let updated = service.get();
        assert_eq!(first.len(), 1);
        assert_eq!(updated.len(), 2);
        assert!(!Arc::ptr_eq(&first.0, &updated.0));

        service.wire.flush().await.unwrap();
    }
}
