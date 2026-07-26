//! Session lifecycle and subagent external-hook adapter.
//!
//! Original:
//! `packages/agent-core-v2/src/session/externalHooks/externalHooksService.ts`,
//! `SessionExternalHooksService`.
//!
//! Rust adaptation: hook futures remain asynchronous and sequential. The
//! source's intentionally unawaited `SubagentStop` promise is a Tokio task
//! owned by this disposable session service.

use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use serde_json::{Map, Value};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            errors::DiError,
            instantiation::ServicesAccessorExt,
            lifecycle::{Disposable, DisposableStore, DisposeResult},
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
    },
    agent::external_hooks::HookMatcherValue,
    app::{
        external_hooks_runner::{
            EXTERNAL_HOOKS_RUNNER_SERVICE_ID, ExternalHooksRunnerServiceHandle,
            ExternalHooksRunnerTriggerArgs,
        },
        session_lifecycle::{
            SESSION_LIFECYCLE_SERVICE_ID, SessionCloseReason, SessionCreateSource,
            SessionLifecycleServiceHandle,
        },
    },
    hooks::{HookRegisterOptions, HookRegistrationError},
    session::{
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        subagent::{
            AgentTaskStartHookContext, AgentTaskStopHookContext, SESSION_SUBAGENT_SERVICE_ID,
            SessionSubagentServiceHandle,
        },
    },
};

use super::{
    SESSION_EXTERNAL_HOOKS_SERVICE_ID, SessionExternalHooksServiceContract,
    SessionExternalHooksServiceHandle,
};

const HOOK_REGISTRATION_ID: &str = "externalHooks";

pub struct SessionExternalHooksService {
    context: SessionContext,
    runner: ExternalHooksRunnerServiceHandle,
    disposables: DisposableStore,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl SessionExternalHooksService {
    // Original: SessionExternalHooksService.constructor(). Rust constructs an
    // Arc first so registered callbacks can hold Weak references to the
    // disposable service instead of borrowing it across async boundaries.
    pub fn new(
        context: SessionContext,
        lifecycle: SessionLifecycleServiceHandle,
        subagents: SessionSubagentServiceHandle,
        runner: ExternalHooksRunnerServiceHandle,
    ) -> Result<Arc<Self>, HookRegistrationError> {
        let service = Arc::new(Self {
            context,
            runner,
            disposables: DisposableStore::new(),
            tasks: Mutex::new(Vec::new()),
        });
        service.install(lifecycle, subagents)?;
        Ok(service)
    }

    fn install(
        self: &Arc<Self>,
        lifecycle: SessionLifecycleServiceHandle,
        subagents: SessionSubagentServiceHandle,
    ) -> Result<(), HookRegistrationError> {
        let weak = Arc::downgrade(self);
        self.disposables
            .add(lifecycle.hooks().on_did_create_session.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |event, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade()
                            && event.session_id == service.context.session_id
                            && event.source != SessionCreateSource::Fork
                        {
                            service.trigger_session_start(event.source).await;
                        }
                        next(event).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(lifecycle.hooks().on_will_close_session.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |event, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade()
                            && event.session_id == service.context.session_id
                        {
                            service.trigger_session_end(event.reason).await;
                        }
                        next(event).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables
            .add(subagents.hooks().on_will_start_agent_task.register(
                HOOK_REGISTRATION_ID,
                Arc::new(move |context, next| {
                    let weak = weak.clone();
                    Box::pin(async move {
                        if let Some(service) = weak.upgrade() {
                            service.run_subagent_start(context).await?;
                        }
                        next(context).await
                    }) as BoxFuture<'_, Result<(), BoxError>>
                }),
                HookRegisterOptions::default(),
            )?);

        let weak = Arc::downgrade(self);
        self.disposables.add(
            subagents
                .on_did_stop_agent_task()
                .subscribe(move |context| {
                    if let Some(service) = weak.upgrade() {
                        service.notify_subagent_stop(context);
                    }
                }),
        );
        Ok(())
    }

    // Original: SessionExternalHooksService.triggerSessionStart(). Forks are
    // filtered by the lifecycle callback before this method is called.
    async fn trigger_session_start(&self, source: SessionCreateSource) {
        let source = session_create_source(source);
        self.runner
            .trigger(
                "SessionStart",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::String(source.into())),
                    cwd: Some(self.context.cwd.clone()),
                    session_id: Some(self.context.session_id.clone()),
                    input_data: Some(Map::from_iter([(
                        "source".into(),
                        Value::String(source.into()),
                    )])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
    }

    // Original: SessionExternalHooksService.triggerSessionEnd().
    async fn trigger_session_end(&self, reason: SessionCloseReason) {
        let reason = session_close_reason(reason);
        self.runner
            .trigger(
                "SessionEnd",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::String(reason.into())),
                    cwd: Some(self.context.cwd.clone()),
                    session_id: Some(self.context.session_id.clone()),
                    input_data: Some(Map::from_iter([(
                        "reason".into(),
                        Value::String(reason.into()),
                    )])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
    }

    // Original: SessionExternalHooksService.runSubagentStart(). Cancellation
    // is checked on both sides of external hook execution.
    async fn run_subagent_start(
        &self,
        context: &AgentTaskStartHookContext,
    ) -> Result<(), BoxError> {
        context
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        self.runner
            .trigger(
                "SubagentStart",
                ExternalHooksRunnerTriggerArgs {
                    matcher_value: Some(HookMatcherValue::String(context.agent_name.clone())),
                    signal: Some(context.signal.clone()),
                    input_data: Some(Map::from_iter([
                        (
                            "agentName".into(),
                            Value::String(context.agent_name.clone()),
                        ),
                        ("prompt".into(), Value::String(context.prompt.clone())),
                    ])),
                    ..ExternalHooksRunnerTriggerArgs::default()
                },
            )
            .await;
        context
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new((*error).clone()) as BoxError)
    }

    // Original: SessionExternalHooksService.notifySubagentStop().
    fn notify_subagent_stop(&self, context: &AgentTaskStopHookContext) {
        let runner = self.runner.clone();
        let context = context.clone();
        let task = tokio::spawn(async move {
            runner
                .fire_and_forget_trigger(
                    "SubagentStop",
                    ExternalHooksRunnerTriggerArgs {
                        matcher_value: Some(HookMatcherValue::String(context.agent_name.clone())),
                        input_data: Some(Map::from_iter([
                            ("agentName".into(), Value::String(context.agent_name)),
                            ("response".into(), Value::String(context.response)),
                        ])),
                        ..ExternalHooksRunnerTriggerArgs::default()
                    },
                )
                .await;
        });
        self.tasks.lock().unwrap().push(task);
    }
}

impl SessionExternalHooksServiceContract for SessionExternalHooksService {}

impl Disposable for SessionExternalHooksService {
    fn dispose(&self) -> DisposeResult {
        let result = self.disposables.dispose();
        for task in self.tasks.lock().unwrap().drain(..) {
            task.abort();
        }
        result
    }
}

fn session_create_source(source: SessionCreateSource) -> &'static str {
    match source {
        SessionCreateSource::Startup => "startup",
        SessionCreateSource::Resume => "resume",
        SessionCreateSource::Fork => "fork",
    }
}

fn session_close_reason(reason: SessionCloseReason) -> &'static str {
    match reason {
        SessionCloseReason::Exit => "exit",
    }
}

// Original: registerScopedService(..., LifecycleScope.Session, Eager,
// "externalHooks").
pub fn register_session_external_hooks_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_EXTERNAL_HOOKS_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let lifecycle = accessor.get(SESSION_LIFECYCLE_SERVICE_ID)?;
            let subagents = accessor.get(SESSION_SUBAGENT_SERVICE_ID)?;
            let runner = accessor.get(EXTERNAL_HOOKS_RUNNER_SERVICE_ID)?;
            let service = SessionExternalHooksService::new(
                (*context).clone(),
                (*lifecycle).clone(),
                (*subagents).clone(),
                (*runner).clone(),
            )
            .map_err(|error| DiError::Factory(error.to_string()))?;
            let service: Arc<dyn SessionExternalHooksServiceContract> = service;
            Ok(SessionExternalHooksServiceHandle(service))
        })
        .disposable(),
        InstantiationType::Eager,
        "externalHooks",
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use crate::{
        _base::{
            di::scope::get_scoped_service_descriptors, event::Event, utils::abort::AbortController,
        },
        agent::external_hooks::{HookBlockDecision, HookResult},
        app::session_lifecycle::{
            CreateChildSessionOptions, CreateSessionOptions, ForkSessionOptions,
            SessionArchivedEvent, SessionClosedEvent, SessionCreatedEvent, SessionForkedEvent,
            SessionLifecycleError, SessionLifecycleHooks, SessionScopeHandle,
            SessionWillCloseEvent,
        },
        session::subagent::{
            AgentRunHandle, AgentRunRequest, AgentTaskHooks, AgentTaskStopEmitter, RunAgentOptions,
            SessionSubagentServiceContract,
        },
    };

    use super::*;

    #[derive(Clone, Debug)]
    struct Call {
        event: String,
        matcher: Option<String>,
        input: Option<Map<String, Value>>,
        cwd: Option<String>,
        session_id: Option<String>,
    }

    struct RecordingRunner {
        calls: Mutex<Vec<Call>>,
        started: AtomicUsize,
    }

    impl RecordingRunner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                started: AtomicUsize::new(0),
            })
        }

        fn record(&self, event: &str, args: ExternalHooksRunnerTriggerArgs) {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.calls.lock().unwrap().push(Call {
                event: event.into(),
                matcher: args.matcher_value.and_then(|matcher| match matcher {
                    HookMatcherValue::String(value) => Some(value),
                    HookMatcherValue::Content(_) => None,
                }),
                input: args.input_data,
                cwd: args.cwd,
                session_id: args.session_id,
            });
        }
    }

    impl Disposable for RecordingRunner {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[async_trait]
    impl crate::app::external_hooks_runner::ExternalHooksRunnerServiceContract for RecordingRunner {
        async fn trigger(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Vec<HookResult> {
            self.record(event, args);
            Vec::new()
        }

        async fn trigger_block(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Option<HookBlockDecision> {
            self.record(event, args);
            None
        }

        async fn fire_and_forget_trigger(
            &self,
            event: &str,
            args: ExternalHooksRunnerTriggerArgs,
        ) -> Vec<HookResult> {
            self.record(event, args);
            Vec::new()
        }
    }

    struct Lifecycle {
        hooks: SessionLifecycleHooks,
    }

    impl Disposable for Lifecycle {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[async_trait]
    impl crate::app::session_lifecycle::SessionLifecycleServiceContract for Lifecycle {
        fn on_did_create_session(&self) -> Event<SessionCreatedEvent> {
            Event::none()
        }
        fn on_did_close_session(&self) -> Event<SessionClosedEvent> {
            Event::none()
        }
        fn on_did_archive_session(&self) -> Event<SessionArchivedEvent> {
            Event::none()
        }
        fn on_did_fork_session(&self) -> Event<SessionForkedEvent> {
            Event::none()
        }
        fn hooks(&self) -> &SessionLifecycleHooks {
            &self.hooks
        }
        async fn create(
            &self,
            _: CreateSessionOptions,
        ) -> Result<SessionScopeHandle, SessionLifecycleError> {
            unreachable!()
        }
        fn get(&self, _: &str) -> Option<SessionScopeHandle> {
            None
        }
        fn list(&self) -> Vec<SessionScopeHandle> {
            Vec::new()
        }
        async fn resume(
            &self,
            _: &str,
        ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError> {
            Ok(None)
        }
        async fn close(&self, _: &str) -> Result<(), SessionLifecycleError> {
            Ok(())
        }
        async fn archive(&self, _: &str) -> Result<(), SessionLifecycleError> {
            Ok(())
        }
        async fn restore(
            &self,
            _: &str,
        ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError> {
            Ok(None)
        }
        async fn fork(
            &self,
            _: ForkSessionOptions,
        ) -> Result<SessionScopeHandle, SessionLifecycleError> {
            unreachable!()
        }
        async fn create_child(
            &self,
            _: CreateChildSessionOptions,
        ) -> Result<SessionScopeHandle, SessionLifecycleError> {
            unreachable!()
        }
    }

    struct Subagents {
        hooks: AgentTaskHooks,
        stopped: AgentTaskStopEmitter,
    }

    impl SessionSubagentServiceContract for Subagents {
        fn hooks(&self) -> &AgentTaskHooks {
            &self.hooks
        }
        fn on_did_stop_agent_task(&self) -> Event<AgentTaskStopHookContext> {
            self.stopped.event()
        }
        fn run(
            &self,
            _: String,
            _: AgentRunRequest,
            _: RunAgentOptions,
        ) -> BoxFuture<'static, Result<AgentRunHandle, BoxError>> {
            Box::pin(async { unreachable!() })
        }
        fn notify_agent_task_stopped(&self, context: AgentTaskStopHookContext) {
            self.stopped.fire(&context);
        }
    }

    fn fixture() -> (
        Arc<SessionExternalHooksService>,
        Arc<Lifecycle>,
        Arc<Subagents>,
        Arc<RecordingRunner>,
    ) {
        let lifecycle = Arc::new(Lifecycle {
            hooks: SessionLifecycleHooks::default(),
        });
        let subagents = Arc::new(Subagents {
            hooks: AgentTaskHooks::default(),
            stopped: AgentTaskStopEmitter::default(),
        });
        let runner = RecordingRunner::new();
        let service = SessionExternalHooksService::new(
            SessionContextInputBuilder::context(),
            SessionLifecycleServiceHandle(lifecycle.clone()),
            SessionSubagentServiceHandle(subagents.clone()),
            ExternalHooksRunnerServiceHandle(runner.clone()),
        )
        .unwrap();
        (service, lifecycle, subagents, runner)
    }

    struct SessionContextInputBuilder;

    impl SessionContextInputBuilder {
        fn context() -> SessionContext {
            crate::session::session_context::make_session_context(
                crate::session::session_context::SessionContextInput {
                    session_id: "session-1".into(),
                    workspace_id: "workspace-1".into(),
                    session_dir: "/sessions/session-1".into(),
                    session_scope: "sessions/session-1".into(),
                    cwd: "/repo".into(),
                    meta_scope: None,
                },
            )
        }
    }

    #[tokio::test]
    async fn lifecycle_hooks_filter_sessions_and_preserve_wire_arguments() {
        let (_service, lifecycle, _subagents, runner) = fixture();
        let handle = crate::_base::di::scope::Scope::create_app(Default::default()).to_handle();
        let mut other = SessionCreatedEvent {
            session_id: "other".into(),
            handle: handle.clone(),
            source: SessionCreateSource::Startup,
        };
        lifecycle
            .hooks
            .on_did_create_session
            .run(&mut other, None)
            .await
            .unwrap();
        let mut fork = SessionCreatedEvent {
            session_id: "session-1".into(),
            handle: handle.clone(),
            source: SessionCreateSource::Fork,
        };
        lifecycle
            .hooks
            .on_did_create_session
            .run(&mut fork, None)
            .await
            .unwrap();
        let mut startup = SessionCreatedEvent {
            session_id: "session-1".into(),
            handle: handle.clone(),
            source: SessionCreateSource::Startup,
        };
        lifecycle
            .hooks
            .on_did_create_session
            .run(&mut startup, None)
            .await
            .unwrap();
        let mut close = SessionWillCloseEvent {
            session_id: "session-1".into(),
            handle,
            reason: SessionCloseReason::Exit,
        };
        lifecycle
            .hooks
            .on_will_close_session
            .run(&mut close, None)
            .await
            .unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].event, "SessionStart");
        assert_eq!(calls[0].matcher.as_deref(), Some("startup"));
        assert_eq!(calls[0].input.as_ref().unwrap()["source"], "startup");
        assert_eq!(calls[0].cwd.as_deref(), Some("/repo"));
        assert_eq!(calls[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(calls[1].event, "SessionEnd");
        assert_eq!(calls[1].matcher.as_deref(), Some("exit"));
        assert_eq!(calls[1].input.as_ref().unwrap()["reason"], "exit");
    }

    #[tokio::test]
    async fn subagent_hooks_check_cancellation_and_emit_start_then_stop() {
        let (_service, _lifecycle, subagents, runner) = fixture();
        let controller = AbortController::new();
        let mut start = AgentTaskStartHookContext {
            agent_name: "coder".into(),
            prompt: "implement".into(),
            signal: controller.signal(),
        };
        subagents
            .hooks
            .on_will_start_agent_task
            .run(&mut start, None)
            .await
            .unwrap();
        subagents.notify_agent_task_stopped(AgentTaskStopHookContext {
            agent_name: "coder".into(),
            response: "done".into(),
        });
        tokio::task::yield_now().await;

        {
            let calls = runner.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].event, "SubagentStart");
            assert_eq!(calls[0].input.as_ref().unwrap()["agentName"], "coder");
            assert_eq!(calls[0].input.as_ref().unwrap()["prompt"], "implement");
            assert_eq!(calls[1].event, "SubagentStop");
            assert_eq!(calls[1].input.as_ref().unwrap()["response"], "done");
        }

        let aborted = AbortController::new();
        aborted.abort(None);
        let mut start = AgentTaskStartHookContext {
            agent_name: "blocked".into(),
            prompt: "never".into(),
            signal: aborted.signal(),
        };
        assert!(
            subagents
                .hooks
                .on_will_start_agent_task
                .run(&mut start, None)
                .await
                .is_err()
        );
        assert_eq!(runner.started.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn registration_is_eager_and_session_scoped() {
        crate::_base::di::scope::clear_scoped_registry_for_tests();
        register_session_external_hooks_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_EXTERNAL_HOOKS_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "externalHooks"
        }));
        crate::_base::di::scope::clear_scoped_registry_for_tests();
    }
}
