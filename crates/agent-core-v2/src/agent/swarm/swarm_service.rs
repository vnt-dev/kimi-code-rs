//! Agent-scope swarm-mode state, reminders, and turn-end lifecycle.
//!
//! Original: `agent/swarm/swarmService.ts`.

use std::{ops::Deref, sync::Arc};

use serde_json::{Map, Value};

use crate::{
    _base::di::{
        descriptors::SyncDescriptor,
        instantiation::{ServiceIdentifier, ServicesAccessorExt},
        lifecycle::{Disposable, DisposableStore, DisposeResult},
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    agent::{
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, AgentContextMemoryServiceContract,
            AgentContextMemoryServiceHandle, PromptOrigin,
        },
        system_reminder::{
            AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceContract,
            AgentSystemReminderServiceHandle,
        },
    },
    app::event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID, EventBusContract, EventBusHandle},
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::{WireService, WireServiceError},
    },
};

use super::{SWARM_MODEL, SwarmModeTrigger, swarm_enter, swarm_exit};

const SWARM_MODE_ENTER_REMINDER: &str = include_str!("enter-reminder.md");
const SWARM_MODE_EXIT_REMINDER: &str = include_str!("exit-reminder.md");

pub trait AgentSwarmServiceContract: Disposable + Send + Sync {
    fn is_active(&self) -> bool;
    fn enter(&self, trigger: SwarmModeTrigger) -> Result<(), SwarmServiceError>;
    fn exit(&self) -> Result<(), SwarmServiceError>;
}

#[derive(Clone)]
pub struct AgentSwarmServiceHandle(pub Arc<dyn AgentSwarmServiceContract>);
impl Deref for AgentSwarmServiceHandle {
    type Target = dyn AgentSwarmServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
impl Disposable for AgentSwarmServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_SWARM_SERVICE_ID: ServiceIdentifier<AgentSwarmServiceHandle> =
    ServiceIdentifier::new("agentSwarmService");

#[derive(Debug, thiserror::Error)]
pub enum SwarmServiceError {
    #[error(transparent)]
    Wire(#[from] WireServiceError),
    #[error(transparent)]
    Context(#[from] crate::agent::context_memory::ContextMemoryServiceError),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
}

pub struct AgentSwarmService {
    wire: Arc<WireService>,
    reminders: Arc<dyn AgentSystemReminderServiceContract>,
    context: Arc<dyn AgentContextMemoryServiceContract>,
    event_bus: Arc<dyn EventBusContract>,
    disposables: DisposableStore,
}

impl AgentSwarmService {
    pub fn new(
        wire: Arc<WireService>,
        reminders: Arc<dyn AgentSystemReminderServiceContract>,
        context: Arc<dyn AgentContextMemoryServiceContract>,
        event_bus: Arc<dyn EventBusContract>,
    ) -> Arc<Self> {
        std::sync::LazyLock::force(&SWARM_MODEL);
        let service = Arc::new(Self {
            wire,
            reminders,
            context,
            event_bus: Arc::clone(&event_bus),
            disposables: DisposableStore::new(),
        });
        let weak = Arc::downgrade(&service);
        service.disposables.add(event_bus.subscribe_type(
            "turn.ended",
            Arc::new(move |_| {
                if let Some(service) = weak.upgrade()
                    && service.should_auto_exit()
                {
                    let _ = service.exit();
                }
            }),
        ));
        service
    }

    pub fn from_handles(
        wire: WireServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        context: AgentContextMemoryServiceHandle,
        event_bus: EventBusHandle,
    ) -> Arc<Self> {
        Self::new(wire.0, reminders.0, context.0, event_bus.0)
    }

    fn trigger(&self) -> Option<SwarmModeTrigger> {
        self.wire.get_model(&SWARM_MODEL)
    }
    fn should_auto_exit(&self) -> bool {
        matches!(
            self.trigger(),
            Some(SwarmModeTrigger::Task | SwarmModeTrigger::Tool)
        )
    }
}

impl AgentSwarmServiceContract for AgentSwarmService {
    fn is_active(&self) -> bool {
        self.trigger().is_some()
    }

    // Original: AgentSwarmService.enter().
    fn enter(&self, trigger: SwarmModeTrigger) -> Result<(), SwarmServiceError> {
        if self.is_active() {
            return Ok(());
        }
        self.wire.dispatch([swarm_enter(trigger)?])?;
        if trigger != SwarmModeTrigger::Tool {
            self.reminders.append_system_reminder(
                SWARM_MODE_ENTER_REMINDER,
                PromptOrigin::Injection {
                    variant: "swarm_mode".into(),
                },
            )?;
        }
        Ok(())
    }

    // Original: AgentSwarmService.exit().
    fn exit(&self) -> Result<(), SwarmServiceError> {
        let Some(trigger) = self.trigger() else {
            return Ok(());
        };
        let history = self.context.get();
        let will_pop = history.last().is_some_and(|message| matches!(&message.origin, Some(PromptOrigin::Injection { variant }) if variant == "swarm_mode"));
        self.wire.dispatch([swarm_exit()?])?;
        if trigger == SwarmModeTrigger::Tool {
            return Ok(());
        }
        if will_pop {
            self.event_bus.publish(DomainEvent::new(
                "context.spliced",
                Map::from_iter([
                    (
                        "start".into(),
                        Value::from(history.len().saturating_sub(1) as u64),
                    ),
                    ("deleteCount".into(), Value::from(1_u64)),
                    ("messages".into(), Value::Array(Vec::new())),
                ]),
            ));
        } else {
            self.reminders.append_system_reminder(
                SWARM_MODE_EXIT_REMINDER,
                PromptOrigin::Injection {
                    variant: "swarm_mode_exit".into(),
                },
            )?;
        }
        Ok(())
    }
}

impl Disposable for AgentSwarmService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

pub fn register_agent_swarm_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_SWARM_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let service: Arc<dyn AgentSwarmServiceContract> = AgentSwarmService::from_handles(
                (*wire).clone(),
                (*reminders).clone(),
                (*context).clone(),
                (*event_bus).clone(),
            );
            Ok(AgentSwarmServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "swarm",
    );
}
