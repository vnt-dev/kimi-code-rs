//! `/init` session service implementation.
//!
//! Original: `packages/agent-core-v2/src/session/sessionInit/sessionInitService.ts`.

use std::{
    error::Error,
    path::Path,
};
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};
use parking_lot::Mutex;

use futures_util::{FutureExt, future::BoxFuture};

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        errors::errors::{Error2, Error2Options, ErrorCause},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::{
            AbortController, is_abort_error, is_user_cancellation, user_cancellation_reason,
        },
    },
    agent::{
        context_memory::PromptOrigin,
        permission_mode::AGENT_PERMISSION_MODE_SERVICE_ID,
        profile::{
            AGENT_PROFILE_SERVICE_ID, BindAgentInput,
            context::{ProfileContextDeps, load_agents_md},
        },
        system_reminder::AGENT_SYSTEM_REMINDER_SERVICE_ID,
    },
    app::bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{HOST_FILE_SYSTEM_SERVICE_ID, HostFileSystemServiceHandle},
    },
    session::{
        agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, AGENT_NOT_FOUND, AgentLifecycleServiceHandle,
            CreateAgentOptions, MAIN_AGENT_ID,
        },
        subagent::{
            AgentRunRequest, AgentRunSpawnedMeta, MirrorAgentRunOptions, RunAgentOptions,
            SESSION_SUBAGENT_SERVICE_ID, SessionSubagentServiceHandle, emit_agent_run_spawned,
            mirror_agent_run,
        },
    },
    wire::contract::WIRE_SERVICE_ID,
};

use super::{
    DEFAULT_INIT_PROMPT, SESSION_INIT_FAILED, SESSION_INIT_SERVICE_ID, SessionInitServiceContract,
    SessionInitServiceHandle, ensure_session_init_errors_registered, init_completion_reminder,
};

const INIT_PROFILE_NAME: &str = "coder";
const INIT_PARENT_TOOL_CALL_ID: &str = "generate-agents-md";
const INIT_DESCRIPTION: &str = "Initialize AGENTS.md";

struct SessionInitInner {
    lifecycle: AgentLifecycleServiceHandle,
    subagents: SessionSubagentServiceHandle,
    fs: HostFileSystemServiceHandle,
    environment: HostEnvironmentHandle,
    bootstrap: BootstrapServiceHandle,
    active: Mutex<Option<(u64, AbortController)>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
pub struct SessionInitService {
    inner: Arc<SessionInitInner>,
}

impl SessionInitService {
    pub fn new(
        lifecycle: AgentLifecycleServiceHandle,
        subagents: SessionSubagentServiceHandle,
        fs: HostFileSystemServiceHandle,
        environment: HostEnvironmentHandle,
        bootstrap: BootstrapServiceHandle,
    ) -> Self {
        ensure_session_init_errors_registered();
        Self {
            inner: Arc::new(SessionInitInner {
                lifecycle,
                subagents,
                fs,
                environment,
                bootstrap,
                active: Mutex::new(None),
                next_generation: AtomicU64::new(1),
            }),
        }
    }

    async fn generate(self) -> Result<(), BoxError> {
        let main = self.inner.lifecycle.get(MAIN_AGENT_ID).ok_or_else(|| {
            Box::new(Error2::new(AGENT_NOT_FOUND, "Main agent was not found")) as BoxError
        })?;

        let generation = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        let controller = AbortController::new();
        *self.inner.active.lock() = Some((generation, controller.clone()));

        let result = self.run(&main, &controller).await;
        let mut active = self.inner.active.lock();
        if active
            .as_ref()
            .is_some_and(|(current, _)| *current == generation)
        {
            *active = None;
        }
        result
    }

    async fn run(
        &self,
        main: &crate::_base::di::scope::ScopeHandle,
        controller: &AbortController,
    ) -> Result<(), BoxError> {
        let result = async {
            let own = main.get(AGENT_PROFILE_SERVICE_ID)?.data()?;
            let model = own.config.model_alias.clone().ok_or_else(|| {
                Box::new(Error2::new(
                    SESSION_INIT_FAILED,
                    "Main agent has no model bound",
                )) as BoxError
            })?;
            let permission_mode = main.get(AGENT_PERMISSION_MODE_SERVICE_ID)?.mode();
            let child = self
                .inner
                .lifecycle
                .create(CreateAgentOptions {
                    binding: Some(BindAgentInput {
                        profile: INIT_PROFILE_NAME.into(),
                        model: Some(model),
                        thinking: Some(own.config.thinking_level.clone()),
                        strict_thinking: None,
                        cwd: Some(own.config.cwd.clone()),
                    }),
                    ..CreateAgentOptions::default()
                })
                .await?;
            child
                .get(AGENT_PERMISSION_MODE_SERVICE_ID)?
                .set_mode(permission_mode)?;

            emit_agent_run_spawned(
                main,
                child.id(),
                &AgentRunSpawnedMeta {
                    profile_name: INIT_PROFILE_NAME.into(),
                    parent_tool_call_id: Some(INIT_PARENT_TOOL_CALL_ID.into()),
                    description: Some(INIT_DESCRIPTION.into()),
                    run_in_background: Some(false),
                    ..AgentRunSpawnedMeta::default()
                },
            );
            let signal = controller.signal();
            let run = self
                .inner
                .subagents
                .run(
                    child.id().into(),
                    AgentRunRequest::Prompt {
                        prompt: DEFAULT_INIT_PROMPT.into(),
                    },
                    RunAgentOptions::new(signal.clone()),
                )
                .await?;
            let cancel = controller.clone();
            mirror_agent_run(
                main,
                run,
                MirrorAgentRunOptions {
                    profile_name: INIT_PROFILE_NAME.into(),
                    prompt: Some(DEFAULT_INIT_PROMPT.into()),
                    suppress_rate_limit_failure_event: false,
                    signal,
                    cancel: Some(Arc::new(move |reason| {
                        cancel.abort(reason.and_then(boxed_abort_reason));
                    })),
                },
            )
            .await?;

            let agents_md = load_agents_md(
                &ProfileContextDeps {
                    fs: self.inner.fs.clone(),
                    home_dir: self.inner.environment.home_dir()?.into(),
                },
                Path::new(&own.config.cwd),
                Some(self.inner.bootstrap.home_dir()),
            )
            .await;
            main.get(AGENT_SYSTEM_REMINDER_SERVICE_ID)?
                .append_system_reminder(
                    &init_completion_reminder(&agents_md),
                    PromptOrigin::Injection {
                        variant: "init".into(),
                    },
                )?;
            main.get(WIRE_SERVICE_ID)?.flush().await?;
            Ok::<(), BoxError>(())
        }
        .await;

        match result {
            Ok(()) => Ok(()),
            Err(error) if error_chain_matches(error.as_ref(), is_user_cancellation) => Err(error),
            Err(error) if error_chain_matches(error.as_ref(), is_abort_error) => Err(error),
            Err(error)
                if error
                    .downcast_ref::<Error2>()
                    .is_some_and(|error| error.code == SESSION_INIT_FAILED) =>
            {
                Err(error)
            }
            Err(error) => {
                let message = error.to_string();
                let cause: Arc<dyn Error + Send + Sync> = Arc::from(error);
                Err(Box::new(Error2::with_options(
                    SESSION_INIT_FAILED,
                    message,
                    Error2Options {
                        cause: Some(ErrorCause::Error(cause)),
                        ..Error2Options::default()
                    },
                )))
            }
        }
    }
}

impl SessionInitServiceContract for SessionInitService {
    fn generate_agents_md(&self) -> BoxFuture<'static, Result<(), BoxError>> {
        self.clone().generate().boxed()
    }

    fn cancel_init(&self) {
        if let Some((_, controller)) = self.inner.active.lock().as_ref() {
            controller.abort(Some(user_cancellation_reason()));
        }
    }
}

fn boxed_abort_reason(error: BoxError) -> Option<crate::_base::utils::abort::AbortError> {
    error
        .downcast::<crate::_base::utils::abort::AbortError>()
        .ok()
        .map(|reason| *reason)
}

fn error_chain_matches(
    mut error: &(dyn Error + 'static),
    predicate: fn(&(dyn Error + 'static)) -> bool,
) -> bool {
    loop {
        if predicate(error) {
            return true;
        }
        let Some(source) = error.source() else {
            return false;
        };
        error = source;
    }
}

pub fn register_session_init_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_INIT_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = Arc::new(SessionInitService::new(
                (*accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_SUBAGENT_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone(),
            ));
            let contract: Arc<dyn SessionInitServiceContract> = service;
            Ok(SessionInitServiceHandle(contract))
        }),
        InstantiationType::Eager,
        "session-init",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::_base::di::scope::{
        clear_scoped_registry_for_tests, get_scoped_service_descriptors,
    };

    #[test]
    fn registration_matches_the_eager_session_scoped_source_binding() {
        clear_scoped_registry_for_tests();
        register_session_init_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_INIT_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "session-init"
        }));
        clear_scoped_registry_for_tests();
    }
}
