//! Loadable-tool announcement bridge for progressive disclosure.
//!
//! Original: `agent/toolSelect/toolSelectAnnouncementsService.ts`.

use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::future::BoxFuture;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::{ServiceIdentifier, ServicesAccessorExt},
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
    },
    agent::{
        context_memory::PromptOrigin,
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle},
        system_reminder::{AGENT_SYSTEM_REMINDER_SERVICE_ID, AgentSystemReminderServiceHandle},
    },
    app::event::event_bus::{EVENT_BUS_SERVICE_ID, EventBusHandle},
    hooks::HookRegisterOptions,
};

use super::{AGENT_TOOL_SELECT_SERVICE_ID, AgentToolSelectServiceHandle, LOADABLE_TOOLS_TRIGGER};

pub trait AgentToolSelectAnnouncementsServiceContract: Disposable + Send + Sync {}

#[derive(Clone)]
pub struct AgentToolSelectAnnouncementsServiceHandle(
    pub Arc<dyn AgentToolSelectAnnouncementsServiceContract>,
);

impl Deref for AgentToolSelectAnnouncementsServiceHandle {
    type Target = dyn AgentToolSelectAnnouncementsServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Disposable for AgentToolSelectAnnouncementsServiceHandle {
    fn dispose(&self) -> DisposeResult {
        self.0.dispose()
    }
}

pub const AGENT_TOOL_SELECT_ANNOUNCEMENTS_SERVICE_ID: ServiceIdentifier<
    AgentToolSelectAnnouncementsServiceHandle,
> = ServiceIdentifier::new("agentToolSelectAnnouncementsService");

pub struct AgentToolSelectAnnouncementsService {
    tool_select: AgentToolSelectServiceHandle,
    reminders: AgentSystemReminderServiceHandle,
    needs_boundary_injection: Arc<AtomicBool>,
    disposables: DisposableStore,
}

impl AgentToolSelectAnnouncementsService {
    pub fn new(
        tool_select: AgentToolSelectServiceHandle,
        reminders: AgentSystemReminderServiceHandle,
        event_bus: EventBusHandle,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<Arc<Self>, crate::hooks::HookRegistrationError> {
        let service = Arc::new(Self {
            tool_select,
            reminders,
            needs_boundary_injection: Arc::new(AtomicBool::new(false)),
            disposables: DisposableStore::new(),
        });
        service.install(event_bus, loop_service)?;
        Ok(service)
    }

    fn install(
        self: &Arc<Self>,
        event_bus: EventBusHandle,
        loop_service: AgentLoopServiceHandle,
    ) -> Result<(), crate::hooks::HookRegistrationError> {
        let needs_boundary_injection = Arc::clone(&self.needs_boundary_injection);
        self.disposables.add(event_bus.subscribe_type(
            "compaction.completed",
            Arc::new(move |_| {
                needs_boundary_injection.store(true, Ordering::SeqCst);
            }),
        ));
        let service = Arc::downgrade(self);
        self.disposables
            .add(loop_service.hooks().on_will_begin_step.register(
                "toolSelectAnnouncements",
                Arc::new(move |context, next| {
                    let service = service.clone();
                    Box::pin(async move {
                        next(context).await?;
                        let Some(service) = service.upgrade() else {
                            return Ok(());
                        };
                        let is_boundary = context.step == 1
                            || service
                                .needs_boundary_injection
                                .swap(false, Ordering::SeqCst);
                        if !is_boundary {
                            return Ok(());
                        }
                        service
                            .needs_boundary_injection
                            .store(false, Ordering::SeqCst);
                        service
                            .inject()
                            .map_err(|error| Box::new(error) as BoxError)
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);
        Ok(())
    }

    // Original: AgentToolSelectAnnouncementsService.inject().
    fn inject(&self) -> Result<(), crate::agent::context_memory::ContextMemoryServiceError> {
        let Some(announcement) = self.tool_select.loadable_tools_announcement() else {
            return Ok(());
        };
        self.reminders.append_system_reminder(
            &announcement,
            PromptOrigin::SystemTrigger {
                name: LOADABLE_TOOLS_TRIGGER.into(),
            },
        )?;
        Ok(())
    }
}

impl Disposable for AgentToolSelectAnnouncementsService {
    fn dispose(&self) -> DisposeResult {
        self.disposables.dispose()
    }
}

impl AgentToolSelectAnnouncementsServiceContract for AgentToolSelectAnnouncementsService {}

pub fn register_agent_tool_select_announcements_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_TOOL_SELECT_ANNOUNCEMENTS_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let tool_select = accessor.get(AGENT_TOOL_SELECT_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let service = AgentToolSelectAnnouncementsService::new(
                (*tool_select).clone(),
                (*reminders).clone(),
                (*event_bus).clone(),
                (*loop_service).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            Ok(AgentToolSelectAnnouncementsServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "toolSelect",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_injection_is_requested_by_compaction_and_consumed_once() {
        let needs_boundary_injection = AtomicBool::new(false);
        assert!(!needs_boundary_injection.swap(false, Ordering::SeqCst));
        needs_boundary_injection.store(true, Ordering::SeqCst);
        assert!(needs_boundary_injection.swap(false, Ordering::SeqCst));
        assert!(!needs_boundary_injection.swap(false, Ordering::SeqCst));
    }
}
