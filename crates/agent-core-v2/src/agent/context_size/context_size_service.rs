use std::{
    ops::Deref,
    sync::{Arc, Mutex},
};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::context_memory::{
        AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract, ContextMessage,
    },
    kosong::contract::{
        message::Message,
        tokens::estimate_tokens_for_messages,
        usage::{TokenUsage, grand_total},
    },
    wire::{
        contract::WIRE_SERVICE_ID,
        wire_service::{WireService, WireServiceError},
    },
};

use super::context_size_ops::{
    CONTEXT_SIZE_MEASURED, CONTEXT_SIZE_MODEL, ContextSizeMeasuredPayload, context_size_measured,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextSize {
    pub size: f64,
    pub measured: f64,
    pub estimated: f64,
}

pub trait AgentContextSizeServiceContract: Send + Sync {
    fn get(&self, start: Option<f64>, end: Option<f64>) -> ContextSize;
    fn measured(
        &self,
        input: &[Message],
        output: &[Message],
        usage: TokenUsage,
    ) -> Result<(), ContextSizeServiceError>;
}

#[derive(Clone)]
pub struct AgentContextSizeServiceHandle(pub Arc<dyn AgentContextSizeServiceContract>);

impl Deref for AgentContextSizeServiceHandle {
    type Target = dyn AgentContextSizeServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_CONTEXT_SIZE_SERVICE_ID: ServiceIdentifier<AgentContextSizeServiceHandle> =
    ServiceIdentifier::new("agentContextSizeService");

#[derive(Debug, thiserror::Error)]
pub enum ContextSizeServiceError {
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Wire(#[from] WireServiceError),
}

pub struct AgentContextSizeService {
    context: Arc<dyn AgentContextMemoryServiceContract>,
    wire: Arc<WireService>,
    last_emitted_tokens: Mutex<f64>,
}

impl AgentContextSizeService {
    pub fn new(
        context: Arc<dyn AgentContextMemoryServiceContract>,
        wire: Arc<WireService>,
    ) -> Self {
        std::sync::LazyLock::force(&CONTEXT_SIZE_MEASURED);
        Self {
            context,
            wire,
            last_emitted_tokens: Mutex::new(0.0),
        }
    }

    fn emit_if_changed(&self) {
        let tokens = self.wire.get_model(&CONTEXT_SIZE_MODEL).tokens;
        let mut last = self.last_emitted_tokens.lock().unwrap();
        if tokens != *last {
            *last = tokens;
        }
    }
}

// Original: registerScopedService(..., AgentContextSizeService, Eager,
// "contextSize").
pub fn register_agent_context_size_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_CONTEXT_SIZE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let service: Arc<dyn AgentContextSizeServiceContract> = Arc::new(
                AgentContextSizeService::new(Arc::clone(&context.0), Arc::clone(&wire.0)),
            );
            Ok(AgentContextSizeServiceHandle(service))
        }),
        InstantiationType::Eager,
        "contextSize",
    );
}

impl AgentContextSizeServiceContract for AgentContextSizeService {
    // Original: contextSizeService.ts, get().
    fn get(&self, start: Option<f64>, end: Option<f64>) -> ContextSize {
        let context = self.context.get();
        let model = self.wire.get_model(&CONTEXT_SIZE_MODEL);
        let context_length = context.len() as f64;
        let measured_length = model.length.min(context_length);
        let from = normalize_slice_index(start.unwrap_or(0.0), context.len());
        let to = normalize_slice_index(end.unwrap_or(context_length), context.len());
        let measured_end = js_min(to, measured_length);
        let estimated_start = js_max(from, measured_length);
        let measured = if from == 0.0 && measured_end == measured_length {
            model.tokens
        } else {
            estimate_context_range(&context, from, measured_end)
        };
        let estimated = estimate_context_range(&context, estimated_start, to);
        ContextSize {
            size: measured + estimated,
            measured,
            estimated,
        }
    }

    // Original: contextSizeService.ts, measured().
    fn measured(
        &self,
        input: &[Message],
        _output: &[Message],
        usage: TokenUsage,
    ) -> Result<(), ContextSizeServiceError> {
        let context = self.context.get();
        if !matches_context(input, &context) {
            return Ok(());
        }
        self.wire
            .dispatch([context_size_measured(ContextSizeMeasuredPayload {
                length: context.len() as f64,
                tokens: grand_total(&usage),
            })?])?;
        self.emit_if_changed();
        Ok(())
    }
}

// Rust adaptation: JavaScript compares message object identity. Wire snapshots
// are owned Rust values, so full Message equality is the stable equivalent and
// still rejects stale or differently shaped request contexts.
fn matches_context(input: &[Message], context: &[ContextMessage]) -> bool {
    input.len() == context.len()
        && input
            .iter()
            .zip(context)
            .all(|(input, context)| input == &context.message)
}

fn estimate_context_range(context: &[ContextMessage], start: f64, end: f64) -> f64 {
    let start = to_slice_index(start, context.len());
    let end = to_slice_index(end, context.len());
    if start >= end {
        return 0.0;
    }
    estimate_tokens_for_messages(context[start..end].iter().map(|message| &message.message)) as f64
}

// Original: contextSizeService.ts, normalizeSliceIndex(). The returned number
// intentionally remains fractional until Array#slice coercion.
fn normalize_slice_index(index: f64, length: usize) -> f64 {
    if index < 0.0 {
        js_max(length as f64 + index, 0.0)
    } else {
        js_min(index, length as f64)
    }
}

fn to_slice_index(index: f64, length: usize) -> usize {
    if index.is_nan() || index == f64::NEG_INFINITY {
        0
    } else if index == f64::INFINITY {
        length
    } else if index < 0.0 {
        ((length as f64 + index.trunc()).max(0.0) as usize).min(length)
    } else {
        (index.trunc() as usize).min(length)
    }
}

fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.min(right)
    }
}

fn js_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.max(right)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use futures_util::stream;
    use serde_json::Value;

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
        agent::context_memory::AgentContextMemoryService,
        app::event::{event_bus::EventBusContract, event_bus_service::EventBusService},
        kosong::contract::message::{ContentPart, Message, Role},
        persistence::interface::append_log_store::{
            AppendLogError, AppendLogOptions, AppendLogStoreHandle, AppendLogStoreService,
            AppendLogValueStream,
        },
        wire::{
            contract::{WIRE_SERVICE_ID, WireServiceHandle},
            wire_service::{DomainEventPublisher, WireBlobService},
        },
    };

    #[derive(Default)]
    struct MemoryLog;

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, _: Value, _: AppendLogOptions) {}
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

    fn setup() -> (Arc<AgentContextMemoryService>, AgentContextSizeService) {
        let events = Arc::new(EventBusService::new());
        let publisher: Arc<dyn DomainEventPublisher> = events.clone();
        let wire = Arc::new(WireService::new(
            "agents/size-test",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            publisher,
        ));
        let bus: Arc<dyn EventBusContract> = events;
        let context = Arc::new(AgentContextMemoryService::new(Arc::clone(&wire), bus));
        let context_contract: Arc<dyn AgentContextMemoryServiceContract> = context.clone();
        let size = AgentContextSizeService::new(context_contract, wire);
        (context, size)
    }

    #[test]
    fn registration_matches_the_eager_agent_scoped_source_binding() {
        clear_scoped_registry_for_tests();
        register_agent_context_size_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Agent);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == AGENT_CONTEXT_SIZE_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "contextSize"
        }));

        let (context, _) = setup();
        let events = Arc::new(EventBusService::new());
        let publisher: Arc<dyn DomainEventPublisher> = events;
        let wire = Arc::new(WireService::new(
            "agents/registered-size-test",
            AppendLogStoreHandle(Arc::new(MemoryLog)),
            Arc::new(IdentityBlobs),
            publisher,
        ));
        let context_contract: Arc<dyn AgentContextMemoryServiceContract> = context;
        let mut extra = ServiceCollection::new();
        extra.set_instance(
            AGENT_CONTEXT_MEMORY_SERVICE_ID,
            Arc::new(
                crate::agent::context_memory::AgentContextMemoryServiceHandle(context_contract),
            ),
        );
        extra.set_instance(
            WIRE_SERVICE_ID,
            Arc::new(WireServiceHandle(Arc::clone(&wire))),
        );
        let app = Scope::create_app(ScopeOptions { id: None, extra });
        let agent = app
            .create_child(LifecycleScope::Agent, "agent", ScopeOptions::default())
            .unwrap();
        assert_eq!(
            agent
                .get(AGENT_CONTEXT_SIZE_SERVICE_ID)
                .unwrap()
                .get(None, None)
                .size,
            0.0
        );
        agent.dispose().unwrap();
        app.dispose().unwrap();
        clear_scoped_registry_for_tests();
    }

    fn context_message(text: &str) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text { text: text.into() }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: None,
            is_error: None,
            note: None,
        }
    }

    #[tokio::test]
    async fn combines_measured_prefix_and_estimated_tail_for_ranges() {
        let (context, size) = setup();
        context
            .append(vec![context_message("abcd"), context_message("efgh")])
            .unwrap();
        size.wire
            .dispatch([context_size_measured(ContextSizeMeasuredPayload {
                length: 1.0,
                tokens: 10.0,
            })
            .unwrap()])
            .unwrap();
        assert_eq!(
            size.get(None, None),
            ContextSize {
                size: 12.0,
                measured: 10.0,
                estimated: 2.0
            }
        );
        assert_eq!(size.get(Some(-1.0), None).measured, 0.0);
        size.wire.flush().await.unwrap();
    }

    #[tokio::test]
    async fn records_usage_only_when_input_matches_current_context() {
        let (context, size) = setup();
        context.append(vec![context_message("hello")]).unwrap();
        let input = context
            .get()
            .into_iter()
            .map(|message| message.message)
            .collect::<Vec<_>>();
        size.measured(
            &input,
            &[],
            TokenUsage {
                input_other: 2.0,
                output: 3.0,
                input_cache_read: 5.0,
                input_cache_creation: 7.0,
            },
        )
        .unwrap();
        assert_eq!(size.get(None, None).measured, 17.0);

        let stale = vec![Message::new(Role::User, Vec::new(), Vec::new())];
        size.measured(
            &stale,
            &[],
            TokenUsage {
                output: 99.0,
                ..TokenUsage::default()
            },
        )
        .unwrap();
        assert_eq!(size.get(None, None).measured, 17.0);
        size.wire.flush().await.unwrap();
    }
}
