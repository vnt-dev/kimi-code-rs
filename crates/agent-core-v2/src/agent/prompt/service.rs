//! Public façade for the per-agent prompt scheduler.
//!
//! Queue ownership and lifecycle transitions live in `scheduler_actor`; this
//! module keeps dependency injection and operations that do not mutate prompt
//! scheduling state.

use std::{error::Error, sync::Arc};

use async_trait::async_trait;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{INSTANTIATION_SERVICE_ID, ServicesAccessorExt},
            instantiation_service::InstantiationService,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        utils::abort::abort_error,
    },
    agent::{
        context_memory::{AGENT_CONTEXT_MEMORY_SERVICE_ID, ContextMessage},
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopServiceHandle, TurnHandle},
        system_reminder::AGENT_SYSTEM_REMINDER_SERVICE_ID,
        tool_executor::{AGENT_TOOL_EXECUTOR_SERVICE_ID, AgentToolExecutorServiceHandle},
    },
    app::event::event_bus::{EVENT_BUS_SERVICE_ID, EventBusHandle},
    kosong::contract::message::{Message, Role},
    wire::contract::{WIRE_SERVICE_ID, WireServiceHandle},
};

use super::{
    contract::{
        AGENT_PROMPT_SERVICE_ID, AgentPromptHooks, AgentPromptServiceContract,
        AgentPromptServiceHandle, PromptHandle, PromptInput, PromptQueueSnapshot,
        PromptServiceResult,
    },
    errors::ensure_prompt_errors_registered,
    scheduler_actor::{
        SchedulerClient, SchedulerRuntime, begin_shutdown, inject_runtime, start_scheduler, undo,
    },
    step_requests::RetryStepRequest,
};

pub struct AgentPromptService {
    runtime: Arc<SchedulerRuntime>,
    scheduler: SchedulerClient,
    hooks: Arc<AgentPromptHooks>,
    disposables: Arc<DisposableStore>,
}

impl AgentPromptService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: crate::agent::context_memory::AgentContextMemoryServiceHandle,
        reminders: crate::agent::system_reminder::AgentSystemReminderServiceHandle,
        instantiation: Arc<InstantiationService>,
        loop_service: AgentLoopServiceHandle,
        tool_executor: AgentToolExecutorServiceHandle,
        wire: WireServiceHandle,
        event_bus: EventBusHandle,
    ) -> Self {
        ensure_prompt_errors_registered();
        let hooks = Arc::new(AgentPromptHooks::default());
        let disposables = Arc::new(DisposableStore::new());
        let runtime = SchedulerRuntime::new(
            context,
            reminders,
            instantiation,
            loop_service,
            wire,
            event_bus,
            Arc::clone(&hooks),
            Arc::clone(&disposables),
        );
        let scheduler = start_scheduler(Arc::clone(&runtime));

        let delivery_runtime = Arc::clone(&runtime);
        let registration = tool_executor
            .hooks()
            .on_did_execute_tool
            .register(
                "prompt-service-delivery",
                Arc::new(move |context, next| {
                    let runtime = Arc::clone(&delivery_runtime);
                    Box::pin(async move {
                        if let Some(delivery) = context.result.delivery.take()
                            && matches!(delivery.kind, crate::tool::ToolDeliveryKind::Steer)
                        {
                            let origin = delivery
                                .message
                                .origin
                                .and_then(|value| serde_json::from_value(value).ok());
                            let message = ContextMessage {
                                message: Message::new(
                                    Role::User,
                                    delivery.message.content,
                                    delivery.message.tool_calls.unwrap_or_default(),
                                ),
                                id: None,
                                provider_message_id: None,
                                origin,
                                is_error: None,
                                note: None,
                                attachments: Vec::new(),
                            };
                            let _ = inject_runtime(runtime, message).await;
                        }
                        next(context).await
                    })
                }),
                Default::default(),
            )
            .expect("prompt-service-delivery hook registration must succeed");
        disposables.add(registration);

        Self {
            runtime,
            scheduler,
            hooks,
            disposables,
        }
    }
}

#[async_trait]
impl AgentPromptServiceContract for AgentPromptService {
    async fn enqueue(&self, input: PromptInput) -> PromptServiceResult<PromptHandle> {
        self.scheduler.enqueue(input).await
    }

    fn list(&self) -> PromptQueueSnapshot {
        self.scheduler.list()
    }

    async fn steer(&self, prompt_ids: &[String]) -> PromptServiceResult<Vec<PromptHandle>> {
        self.scheduler.steer(prompt_ids).await
    }

    async fn abort(&self, prompt_id: &str, reason: Option<Arc<dyn Error + Send + Sync>>) -> bool {
        self.scheduler.abort(prompt_id, reason).await
    }

    async fn inject(&self, message: ContextMessage) -> PromptServiceResult<Option<TurnHandle>> {
        inject_runtime(Arc::clone(&self.runtime), message).await
    }

    async fn retry(&self) -> PromptServiceResult<Option<TurnHandle>> {
        if self.runtime.shutdown.is_cancelled() {
            return Err(Box::new(abort_error(Some(
                "Agent prompt service shut down",
            ))));
        }
        Ok(self
            .runtime
            .loop_service
            .enqueue(Arc::new(RetryStepRequest::new()), None)?
            .assigned
            .await?
            .turn
            .into())
    }

    fn undo(&self, count: u32) -> PromptServiceResult<usize> {
        undo(&self.runtime, count)
    }

    async fn clear(&self) -> PromptServiceResult<()> {
        self.scheduler.clear().await
    }

    async fn shutdown(&self) {
        begin_shutdown(&self.runtime);
        self.runtime.tasks.wait().await;
    }

    fn hooks(&self) -> &AgentPromptHooks {
        &self.hooks
    }
}

impl Disposable for AgentPromptService {
    fn dispose(&self) -> DisposeResult {
        // `dispose` is intentionally non-blocking.  Cancellation wakes the
        // actor, which settles every Deferred before the tracked tasks drain.
        begin_shutdown(&self.runtime);
        self.disposables.dispose()
    }
}

pub fn register_agent_prompt_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_PROMPT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
            let reminders = accessor.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?;
            let instantiation = accessor.get(INSTANTIATION_SERVICE_ID)?;
            let loop_service = accessor.get(AGENT_LOOP_SERVICE_ID)?;
            let executor = accessor.get(AGENT_TOOL_EXECUTOR_SERVICE_ID)?;
            let wire = accessor.get(WIRE_SERVICE_ID)?;
            let event_bus = accessor.get(EVENT_BUS_SERVICE_ID)?;
            let service: Arc<dyn AgentPromptServiceContract> = Arc::new(AgentPromptService::new(
                (*context).clone(),
                (*reminders).clone(),
                instantiation,
                (*loop_service).clone(),
                (*executor).clone(),
                (*wire).clone(),
                (*event_bus).clone(),
            ));
            Ok(AgentPromptServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "prompt",
    );
}
