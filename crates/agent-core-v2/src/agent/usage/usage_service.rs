use std::sync::{Arc, Mutex};

use serde_json::Map;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
    },
    agent::llm_requester::AgentLlmRequestSource,
    app::event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle},
    kosong::contract::usage::{TokenUsage, add_usage},
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::{WireService, WireServiceError},
    },
};

use super::{
    AGENT_USAGE_SERVICE_ID, AgentUsageServiceContract, AgentUsageServiceHandle, RECORD_USAGE,
    RecordUsagePayload, USAGE_MODEL, UsageRecordScope, UsageRecordedContext, UsageStatus,
    record_usage, usage_status_from_state,
};

#[derive(Debug, thiserror::Error)]
pub enum UsageServiceError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
}

#[derive(Default)]
struct CurrentTurnState {
    turn_id: Option<f64>,
    usage: Option<TokenUsage>,
}

pub struct AgentUsageService {
    wire: Arc<WireService>,
    event_bus: Option<Arc<dyn EventBusContract>>,
    on_did_record: Arc<Emitter<UsageRecordedContext>>,
    current_turn: Mutex<CurrentTurnState>,
}

impl AgentUsageService {
    pub fn new(wire: Arc<WireService>, event_bus: Option<Arc<dyn EventBusContract>>) -> Self {
        std::sync::LazyLock::force(&RECORD_USAGE);
        Self {
            wire,
            event_bus,
            on_did_record: Arc::new(Emitter::new()),
            current_turn: Mutex::new(CurrentTurnState::default()),
        }
    }

    pub fn from_handles(wire: WireServiceHandle, event_bus: EventBusHandle) -> Self {
        Self::new(wire.0, Some(event_bus.0))
    }

    fn update_current_turn(&self, source: Option<&AgentLlmRequestSource>, usage: TokenUsage) {
        let Some(AgentLlmRequestSource::Turn { turn_id, .. }) = source else {
            return;
        };
        let mut current = self.current_turn.lock().unwrap();
        if current.turn_id != Some(*turn_id) {
            current.turn_id = Some(*turn_id);
            current.usage = Some(usage);
        } else {
            current.usage = Some(
                current
                    .usage
                    .map(|value| add_usage(&value, &usage))
                    .unwrap_or(usage),
            );
        }
    }
}

impl AgentUsageServiceContract for AgentUsageService {
    // Original: usageService.ts, AgentUsageService.record(). Dispatch and all
    // live notifications deliberately remain synchronous and ordered.
    fn record(
        &self,
        model: String,
        usage: TokenUsage,
        source: Option<AgentLlmRequestSource>,
    ) -> Result<(), UsageServiceError> {
        let usage_scope = if matches!(source, Some(AgentLlmRequestSource::Turn { .. })) {
            UsageRecordScope::Turn
        } else {
            UsageRecordScope::Session
        };
        self.wire.dispatch([record_usage(RecordUsagePayload {
            model: model.clone(),
            usage,
            usage_scope: Some(usage_scope),
        })?])?;

        self.update_current_turn(source.as_ref(), usage);
        if let Some(event_bus) = &self.event_bus {
            event_bus.publish(DomainEvent::new(
                "agent.status.updated",
                Map::from_iter([("usage".into(), serde_json::to_value(self.status())?)]),
            ));
        }
        self.on_did_record.fire(&UsageRecordedContext {
            model,
            usage,
            source,
        });
        Ok(())
    }

    // Original: usageService.ts, AgentUsageService.status().
    fn status(&self) -> UsageStatus {
        let current_turn = self.current_turn.lock().unwrap().usage;
        usage_status_from_state(&self.wire.get_model(&USAGE_MODEL), current_turn.as_ref())
    }

    fn on_did_record(&self) -> Event<UsageRecordedContext> {
        self.on_did_record.event()
    }
}

impl Disposable for AgentUsageService {
    fn dispose(&self) -> DisposeResult {
        self.on_did_record.dispose()
    }
}

impl Drop for AgentUsageService {
    fn drop(&mut self) {
        let _ = self.dispose();
    }
}

// Original: usageService.ts, Agent-scope eager registration.
pub fn register_usage_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_USAGE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let service: Arc<dyn AgentUsageServiceContract> = Arc::new(
                AgentUsageService::from_handles((*wire).clone(), (*event_bus).clone()),
            );
            Ok(AgentUsageServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "usage",
    );
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::di::lifecycle::{DisposableHandle, disposable_none},
        app::event::event_bus::{DomainEventHandler, EventBusContract},
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

    #[derive(Default)]
    struct CaptureEvents(Mutex<Vec<DomainEvent>>);

    impl EventBusContract for CaptureEvents {
        fn publish(&self, event: DomainEvent) {
            self.0.lock().unwrap().push(event);
        }
        fn subscribe(&self, _: DomainEventHandler) -> DisposableHandle {
            disposable_none()
        }
        fn subscribe_type(&self, _: &str, _: DomainEventHandler) -> DisposableHandle {
            disposable_none()
        }
    }

    impl DomainEventPublisher for CaptureEvents {
        fn publish(&self, _: Value) {}
    }

    fn usage(value: f64) -> TokenUsage {
        TokenUsage {
            input_other: value,
            output: value * 2.0,
            input_cache_read: value * 3.0,
            input_cache_creation: value * 4.0,
        }
    }

    fn turn(turn_id: f64) -> AgentLlmRequestSource {
        AgentLlmRequestSource::Turn {
            turn_id,
            step: None,
            log_fields: None,
        }
    }

    fn setup() -> (
        Arc<WireService>,
        Arc<MemoryLog>,
        Arc<CaptureEvents>,
        AgentUsageService,
    ) {
        let log = Arc::new(MemoryLog::default());
        let events = Arc::new(CaptureEvents::default());
        let publisher: Arc<dyn DomainEventPublisher> = events.clone();
        let wire = Arc::new(WireService::new(
            "agents/usage-test",
            AppendLogStoreHandle(log.clone()),
            Arc::new(IdentityBlobs),
            publisher,
        ));
        let bus: Arc<dyn EventBusContract> = events.clone();
        let service = AgentUsageService::new(wire.clone(), Some(bus));
        (wire, log, events, service)
    }

    #[tokio::test]
    async fn records_session_and_turn_usage_with_live_turn_reset_and_ordered_notifications() {
        let (wire, log, events, service) = setup();
        let records = Arc::new(Mutex::new(Vec::new()));
        let captured = records.clone();
        let _subscription = service.on_did_record().subscribe(move |record| {
            captured.lock().unwrap().push(record.clone());
        });

        service.record("a".into(), usage(1.0), None).unwrap();
        assert_eq!(service.status().current_turn, None);
        service
            .record("a".into(), usage(2.0), Some(turn(7.0)))
            .unwrap();
        service
            .record("b".into(), usage(3.0), Some(turn(7.0)))
            .unwrap();
        assert_eq!(service.status().current_turn, Some(usage(5.0)));
        service
            .record("a".into(), usage(4.0), Some(turn(8.0)))
            .unwrap();

        let status = service.status();
        assert_eq!(status.by_model.as_ref().unwrap()["a"], usage(7.0));
        assert_eq!(status.by_model.as_ref().unwrap()["b"], usage(3.0));
        assert_eq!(status.total, Some(usage(10.0)));
        assert_eq!(status.current_turn, Some(usage(4.0)));
        assert_eq!(records.lock().unwrap().len(), 4);
        assert_eq!(events.0.lock().unwrap().len(), 4);
        assert_eq!(
            events.0.lock().unwrap()[3].fields["usage"]["currentTurn"]["output"],
            8.0
        );

        wire.flush().await.unwrap();
        let persisted = log.0.lock().unwrap();
        assert_eq!(persisted.len(), 4);
        assert_eq!(persisted[0]["usageScope"], "session");
        assert_eq!(persisted[1]["usageScope"], "turn");
    }

    #[test]
    fn operation_source_is_session_scoped_and_does_not_change_current_turn() {
        let (_, _, _, service) = setup();
        service
            .record("a".into(), usage(1.0), Some(turn(1.0)))
            .unwrap();
        let operation = AgentLlmRequestSource::Operation {
            turn_id: Some(1.0),
            request_kind: Some("compaction".into()),
            log_fields: None,
        };
        service
            .record("a".into(), usage(2.0), Some(operation))
            .unwrap();
        assert_eq!(service.status().current_turn, Some(usage(1.0)));
    }

    #[test]
    fn disposal_stops_record_listener_delivery() {
        let (_, _, _, service) = setup();
        let count = Arc::new(Mutex::new(0));
        let captured = count.clone();
        let _subscription = service
            .on_did_record()
            .subscribe(move |_| *captured.lock().unwrap() += 1);
        service.dispose().unwrap();
        service.record("a".into(), usage(1.0), None).unwrap();
        assert_eq!(*count.lock().unwrap(), 0);
    }
}
