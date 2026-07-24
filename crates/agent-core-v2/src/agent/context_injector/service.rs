//! Dynamic context-injection service.
//!
//! Original: `packages/agent-core-v2/src/agent/contextInjector/contextInjectorService.ts`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use async_trait::async_trait;
use futures_util::future::BoxFuture;

use super::{
    AGENT_CONTEXT_INJECTOR_SERVICE_ID, AgentContextInjectorServiceContract,
    AgentContextInjectorServiceHandle, ContextInjectionContent, ContextInjectionContext,
    ContextInjectionError, ContextInjectionProvider,
};
use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::ServicesAccessorExt,
            lifecycle::{
                Disposable, DisposableHandle, DisposableStore, DisposeResult, to_disposable,
            },
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle, ContextMessage, PromptOrigin,
            vacuous_content::trim_ecmascript_whitespace,
        },
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, BeforeStepContext},
        system_reminder::{
            AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceContract,
            AgentSystemReminderServiceHandle,
        },
    },
    app::event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusHandle},
    hooks::HookRegisterOptions,
    kosong::contract::message::{Message, Role},
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

#[derive(Clone)]
struct Entry {
    id: u64,
    name: String,
    provider: ContextInjectionProvider,
    positions: Vec<usize>,
}
#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    is_new_turn: bool,
    next_id: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ContextInjectorServiceError {
    #[error(transparent)]
    Hook(#[from] crate::hooks::HookRegistrationError),
}

pub struct AgentContextInjectorService {
    context: Arc<dyn AgentContextMemoryServiceContract>,
    reminders: Arc<dyn AgentSystemReminderServiceContract>,
    state: Arc<Mutex<State>>,
    disposables: DisposableStore,
}

impl AgentContextInjectorService {
    pub fn new(
        context: Arc<dyn AgentContextMemoryServiceContract>,
        loop_service: AgentLoopServiceHandle,
        reminders: Arc<dyn AgentSystemReminderServiceContract>,
        event_bus: EventBusHandle,
        wire: WireServiceHandle,
    ) -> Result<Arc<Self>, ContextInjectorServiceError> {
        let service = Arc::new(Self {
            context,
            reminders,
            state: Arc::new(Mutex::new(State {
                is_new_turn: true,
                ..State::default()
            })),
            disposables: DisposableStore::new(),
        });
        service.install_hooks(loop_service, event_bus, wire)?;
        Ok(service)
    }
    pub fn from_handles(
        context: AgentContextMemoryServiceHandle,
        loop_service: AgentLoopServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        event_bus: EventBusHandle,
        wire: WireServiceHandle,
    ) -> Result<Arc<Self>, ContextInjectorServiceError> {
        Self::new(context.0, loop_service, reminders.0, event_bus, wire)
    }
    fn install_hooks(
        self: &Arc<Self>,
        loop_service: AgentLoopServiceHandle,
        event_bus: EventBusHandle,
        wire: WireServiceHandle,
    ) -> Result<(), ContextInjectorServiceError> {
        let weak = Arc::downgrade(self);
        let hook = loop_service.hooks().on_will_begin_step.register(
            "context-injector",
            Arc::new(move |context: &mut BeforeStepContext, next| {
                let weak = Weak::clone(&weak);
                Box::pin(async move {
                    next(context).await?;
                    if let Some(service) = weak.upgrade() {
                        service.inject().await?;
                    }
                    Ok(())
                }) as BoxFuture<'_, Result<(), BoxError>>
            }),
            HookRegisterOptions::default(),
        )?;
        self.disposables.add(hook);
        let weak = Arc::downgrade(self);
        self.disposables.add(event_bus.subscribe_type(
            "turn.started",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade() {
                    service.state.lock().unwrap().is_new_turn = true;
                }
            }),
        ));
        let weak = Arc::downgrade(self);
        self.disposables.add(event_bus.subscribe_type(
            "context.spliced",
            Arc::new(move |event| {
                if let Some(service) = weak.upgrade()
                    && let Some(splice) = Splice::from_event(event)
                {
                    service.handle_splice(splice);
                }
            }),
        ));
        let weak = Arc::downgrade(self);
        let restore = wire.hooks().on_did_restore.register(
            "context-injector",
            Arc::new(move |context, next| {
                let weak = Weak::clone(&weak);
                Box::pin(async move {
                    if let Some(service) = weak.upgrade() {
                        service.resync_positions();
                    }
                    next(context).await
                })
            }),
            HookRegisterOptions::default(),
        )?;
        self.disposables.add(restore);
        Ok(())
    }
    async fn inject(&self) -> Result<(), ContextInjectionError> {
        let (is_new_turn, entries) = {
            let mut state = self.state.lock().unwrap();
            let value = state.is_new_turn;
            state.is_new_turn = false;
            (value, state.entries.clone())
        };
        for entry in entries {
            let content = (entry.provider)(ContextInjectionContext {
                injected_positions: entry.positions.clone(),
                last_injected_at: entry.positions.last().copied(),
                is_new_turn,
            })
            .await?;
            if !self
                .state
                .lock()
                .unwrap()
                .entries
                .iter()
                .any(|current| current.id == entry.id)
            {
                continue;
            }
            let Some(content) = content else { continue };
            let origin = PromptOrigin::Injection {
                variant: entry.name,
            };
            match content {
                ContextInjectionContent::Text(text) => {
                    if !trim_ecmascript_whitespace(&text).is_empty() {
                        self.reminders.append_system_reminder(&text, origin)?;
                    }
                }
                ContextInjectionContent::Parts(parts) => {
                    if !parts.is_empty() {
                        self.context.append(vec![ContextMessage {
                            message: Message::new(Role::User, parts, Vec::new()),
                            id: None,
                            provider_message_id: None,
                            origin: Some(origin),
                            is_error: None,
                            note: None,
                        }])?;
                    }
                }
            }
        }
        Ok(())
    }
    fn resync_positions(&self) {
        let history = self.context.get();
        for entry in &mut self.state.lock().unwrap().entries {
            entry.positions = find_injections(&history, &entry.name);
        }
    }
    fn handle_splice(&self, splice: Splice) {
        let adopted = splice.injections();
        if adopted.is_empty() && splice.delete_count == 0 {
            return;
        }
        let end = splice.start.saturating_add(splice.delete_count);
        let delta = splice.messages.len() as isize - splice.delete_count as isize;
        for entry in &mut self.state.lock().unwrap().entries {
            let insert = adopted.get(&entry.name).cloned().unwrap_or_default();
            let lo = entry.positions.partition_point(|p| *p < splice.start);
            let hi = entry.positions.partition_point(|p| *p < end);
            for p in &mut entry.positions[hi..] {
                *p = p.saturating_add_signed(delta);
            }
            entry.positions.splice(lo..hi, insert);
        }
    }
}
#[async_trait]
impl AgentContextInjectorServiceContract for AgentContextInjectorService {
    fn register(&self, name: String, provider: ContextInjectionProvider) -> DisposableHandle {
        let id = {
            let mut state = self.state.lock().unwrap();
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            let positions = find_injections(&self.context.get(), &name);
            state.entries.push(Entry {
                id,
                name,
                provider,
                positions,
            });
            id
        };
        let state = Arc::downgrade(&self.state);
        to_disposable(move || {
            if let Some(state) = state.upgrade() {
                state.lock().unwrap().entries.retain(|entry| entry.id != id);
            }
        })
    }
    async fn inject_after_compaction(&self) -> Result<(), ContextInjectionError> {
        self.state.lock().unwrap().is_new_turn = true;
        self.inject().await
    }
}
impl Disposable for AgentContextInjectorService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

struct Splice {
    start: usize,
    delete_count: usize,
    messages: Vec<ContextMessage>,
}
impl Splice {
    fn from_event(event: &DomainEvent) -> Option<Self> {
        Some(Self {
            start: event.fields.get("start")?.as_u64()?.try_into().ok()?,
            delete_count: event.fields.get("deleteCount")?.as_u64()?.try_into().ok()?,
            messages: serde_json::from_value(event.fields.get("messages")?.clone()).ok()?,
        })
    }
    fn injections(&self) -> HashMap<String, Vec<usize>> {
        let mut result = HashMap::new();
        for (offset, message) in self.messages.iter().enumerate() {
            if let Some(PromptOrigin::Injection { variant }) = &message.origin {
                result
                    .entry(variant.clone())
                    .or_insert_with(Vec::new)
                    .push(self.start.saturating_add(offset));
            }
        }
        result
    }
}
fn find_injections(history: &[ContextMessage], variant: &str) -> Vec<usize> {
    history.iter().enumerate().filter_map(|(index, message)| matches!(&message.origin, Some(PromptOrigin::Injection { variant: found }) if found == variant).then_some(index)).collect()
}

pub fn register_agent_context_injector_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_CONTEXT_INJECTOR_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let service = AgentContextInjectorService::from_handles(
                (*context).clone(),
                (*loop_service).clone(),
                (*reminders).clone(),
                (*event_bus).clone(),
                (*wire).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            Ok(AgentContextInjectorServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "contextInjector",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kosong::contract::message::ContentPart;
    fn message(origin: Option<PromptOrigin>) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                Role::User,
                vec![ContentPart::Text { text: "x".into() }],
                vec![],
            ),
            id: None,
            provider_message_id: None,
            origin,
            is_error: None,
            note: None,
        }
    }
    #[test]
    fn finds_existing_and_spliced_injections() {
        let messages = vec![
            message(None),
            message(Some(PromptOrigin::Injection {
                variant: "goal".into(),
            })),
            message(Some(PromptOrigin::Injection {
                variant: "goal".into(),
            })),
        ];
        assert_eq!(find_injections(&messages, "goal"), [1, 2]);
        let splice = Splice {
            start: 3,
            delete_count: 0,
            messages: vec![message(Some(PromptOrigin::Injection {
                variant: "goal".into(),
            }))],
        };
        assert_eq!(splice.injections().get("goal"), Some(&vec![3]));
    }
}
