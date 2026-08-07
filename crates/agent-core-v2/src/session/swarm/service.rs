//! Session-scoped swarm batch implementation.
//!
//! Original: `packages/agent-core-v2/src/session/swarm/sessionSwarmService.ts`.

use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::{FutureExt, future::BoxFuture};
use serde_json::Value;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, ScopeHandle, register_scoped_service},
        },
        lifecycle::lifecycle_machine::BoxError,
        log::{LOG_SERVICE_ID, LogServiceHandle, Logger},
        utils::abort::{AbortController, link_abort_signal},
    },
    agent::{
        loop_::{AGENT_LOOP_SERVICE_ID, AgentLoopState},
        permission_mode::AGENT_PERMISSION_MODE_SERVICE_ID,
        profile::{AGENT_PROFILE_SERVICE_ID, BindAgentInput, ProfileUpdateData},
        user_tool::AGENT_USER_TOOL_SERVICE_ID,
    },
    app::{
        agent_profile_catalog::{
            AgentProfilePromptPrefixContext, apply_profile_prompt_prefix, subagent_model_alias,
        },
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        event::event_bus::{DomainEvent, EVENT_BUS_SERVICE_ID},
    },
    session::{
        agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceHandle, CreateAgentOptions,
            is_subagent_meta, subagent_labels, subagent_parent_agent_id, subagent_swarm_item,
        },
        agent_profile_catalog::{
            SESSION_AGENT_PROFILE_CATALOG_ID, SessionAgentProfileCatalogHandle,
        },
        process::{SESSION_PROCESS_RUNNER_SERVICE_ID, SessionProcessRunnerHandle},
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        session_metadata::{AgentMeta, SESSION_METADATA_ID, SessionMetadataHandle},
        subagent::{
            AgentRunRequest, AgentRunSpawnedMeta, MirrorAgentRunOptions, RunAgentOptions,
            SESSION_SUBAGENT_SERVICE_ID, SessionSubagentServiceHandle, emit_agent_run_spawned,
            mirror_agent_run,
        },
    },
};

use super::{
    AgentRunAttemptHandle, AgentRunAttemptOptions, AgentRunBatch, AgentRunBatchLauncher,
    AgentRunBatchOptions, AgentRunCompletion, AgentRunSuspendedEvent, AgentSpawnAttemptOptions,
    SESSION_SWARM_SERVICE_ID, SessionSwarmFuture, SessionSwarmRunArgs, SessionSwarmServiceContract,
    SessionSwarmServiceHandle, SessionSwarmTask, resolve_swarm_max_concurrency,
};

const RESUMED_PROFILE_FALLBACK: &str = "subagent";

struct InFlight {
    generation: u64,
    controller: AbortController,
}

struct SessionSwarmInner {
    lifecycle: AgentLifecycleServiceHandle,
    subagents: SessionSubagentServiceHandle,
    catalog: SessionAgentProfileCatalogHandle,
    session_context: SessionContext,
    metadata: SessionMetadataHandle,
    process_runner: SessionProcessRunnerHandle,
    log: LogServiceHandle,
    bootstrap: BootstrapServiceHandle,
    in_flight: Mutex<std::collections::HashMap<String, InFlight>>,
    next_generation: AtomicU64,
}

#[derive(Clone)]
pub struct SessionSwarmService {
    inner: Arc<SessionSwarmInner>,
}

impl SessionSwarmService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lifecycle: AgentLifecycleServiceHandle,
        subagents: SessionSubagentServiceHandle,
        catalog: SessionAgentProfileCatalogHandle,
        session_context: SessionContext,
        metadata: SessionMetadataHandle,
        process_runner: SessionProcessRunnerHandle,
        log: LogServiceHandle,
        bootstrap: BootstrapServiceHandle,
    ) -> Self {
        Self {
            inner: Arc::new(SessionSwarmInner {
                lifecycle,
                subagents,
                catalog,
                session_context,
                metadata,
                process_runner,
                log,
                bootstrap,
                in_flight: Mutex::new(std::collections::HashMap::new()),
                next_generation: AtomicU64::new(1),
            }),
        }
    }

    async fn agent_meta(&self, agent_id: &str) -> Result<Option<AgentMeta>, BoxError> {
        Ok(self
            .inner
            .metadata
            .read()
            .await?
            .agents
            .and_then(|agents| agents.get(agent_id).cloned()))
    }

    fn require_handle(&self, agent_id: &str, label: &str) -> Result<ScopeHandle, BoxError> {
        self.inner.lifecycle.get(agent_id).ok_or_else(|| {
            Box::new(io::Error::other(format!(
                "{label} \"{agent_id}\" does not exist"
            ))) as BoxError
        })
    }

    async fn require_owned_subagent(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
    ) -> Result<(), BoxError> {
        let meta = self.agent_meta(agent_id).await?;
        if !is_subagent_meta(meta.as_ref()) {
            return Err(Box::new(io::Error::other(format!(
                "Agent instance \"{agent_id}\" is not a subagent"
            ))));
        }
        if subagent_parent_agent_id(meta.as_ref()).as_deref() != Some(caller_agent_id) {
            return Err(Box::new(io::Error::other(format!(
                "Agent instance \"{agent_id}\" does not belong to this parent agent"
            ))));
        }
        Ok(())
    }

    fn require_idle_subagent(&self, agent_id: &str, child: &ScopeHandle) -> Result<(), BoxError> {
        if child.get(AGENT_LOOP_SERVICE_ID)?.status().state == AgentLoopState::Running {
            return Err(Box::new(io::Error::other(format!(
                "Agent instance \"{agent_id}\" is already running and cannot run concurrently"
            ))));
        }
        Ok(())
    }

    fn realign_child_model(
        &self,
        caller: &ScopeHandle,
        child: &ScopeHandle,
    ) -> Result<(), BoxError> {
        let caller_model = caller
            .get(AGENT_PROFILE_SERVICE_ID)?
            .data()?
            .config
            .model_alias
            .ok_or_else(|| {
                Box::new(io::Error::other("Caller agent has no model bound")) as BoxError
            })?;
        let child_profile_name = child
            .get(AGENT_PROFILE_SERVICE_ID)?
            .data()?
            .config
            .profile_name;
        let child_profile = child_profile_name
            .as_deref()
            .and_then(|name| self.inner.catalog.get(name));
        child
            .get(AGENT_PROFILE_SERVICE_ID)?
            .update(ProfileUpdateData {
                model_alias: Some(subagent_model_alias(child_profile.as_deref(), caller_model)),
                ..ProfileUpdateData::default()
            })?;
        Ok(())
    }

    async fn spawn_attempt(
        &self,
        caller_agent_id: &str,
        options: AgentSpawnAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        options
            .attempt
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        let caller = self.require_handle(caller_agent_id, "Caller agent")?;
        self.inner.catalog.ready().await?;
        let profile = self
            .inner
            .catalog
            .get(&options.profile_name)
            .ok_or_else(|| {
                Box::new(io::Error::other(format!(
                    "Unknown agent type: \"{}\"",
                    options.profile_name
                ))) as BoxError
            })?;
        let caller_data = caller.get(AGENT_PROFILE_SERVICE_ID)?.data()?;
        let caller_model = caller_data.config.model_alias.clone().ok_or_else(|| {
            Box::new(io::Error::other("Caller agent has no model bound")) as BoxError
        })?;
        let model = subagent_model_alias(Some(profile.as_ref()), caller_model);
        let child = self
            .inner
            .lifecycle
            .create(CreateAgentOptions {
                binding: Some(BindAgentInput {
                    profile: profile.name.clone(),
                    model: Some(model),
                    thinking: Some(caller_data.config.thinking_level.clone()),
                    strict_thinking: None,
                    cwd: Some(caller_data.config.cwd.clone()),
                }),
                labels: Some(subagent_labels(
                    caller_agent_id,
                    options.swarm_item.as_deref(),
                )),
                ..CreateAgentOptions::default()
            })
            .await?;
        child
            .get(AGENT_PERMISSION_MODE_SERVICE_ID)?
            .set_mode(caller.get(AGENT_PERMISSION_MODE_SERVICE_ID)?.mode())?;
        child
            .get(AGENT_USER_TOOL_SERVICE_ID)?
            .inherit_user_tools(&caller.get(AGENT_USER_TOOL_SERVICE_ID)?.0)
            .await
            .map_err(|error| Box::new(io::Error::other(error)) as BoxError)?;
        emit_agent_run_spawned(
            &caller,
            child.id(),
            &spawned_meta(&options.attempt, &options.profile_name),
        );
        let logger: Arc<dyn Logger> = self.inner.log.0.clone();
        let prompt = apply_profile_prompt_prefix(
            &profile,
            &options.attempt.prompt,
            AgentProfilePromptPrefixContext {
                cwd: self.inner.session_context.cwd.clone(),
                runner: self.inner.process_runner.clone(),
                log: Some(logger),
            },
        )
        .await;
        self.observe(
            caller,
            child.id().into(),
            options.profile_name,
            AgentRunRequest::Prompt { prompt },
            options.attempt,
        )
        .await
    }

    async fn resume_attempt(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
        options: AgentRunAttemptOptions,
        retry_turn: bool,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        options
            .signal
            .throw_if_aborted()
            .map_err(|error| Box::new((*error).clone()) as BoxError)?;
        self.require_owned_subagent(caller_agent_id, agent_id)
            .await?;
        let caller = self.require_handle(caller_agent_id, "Caller agent")?;
        let child = self.require_handle(agent_id, "Agent instance")?;
        self.require_idle_subagent(agent_id, &child)?;
        self.realign_child_model(&caller, &child)?;
        let profile_name = child
            .get(AGENT_PROFILE_SERVICE_ID)?
            .data()?
            .config
            .profile_name
            .unwrap_or_else(|| RESUMED_PROFILE_FALLBACK.into());
        if !retry_turn {
            emit_agent_run_spawned(&caller, agent_id, &spawned_meta(&options, &profile_name));
        }
        let request = if retry_turn {
            AgentRunRequest::Retry { trigger: None }
        } else {
            AgentRunRequest::Prompt {
                prompt: options.prompt.clone(),
            }
        };
        self.observe(caller, agent_id.into(), profile_name, request, options)
            .await
    }

    async fn observe(
        &self,
        caller: ScopeHandle,
        agent_id: String,
        profile_name: String,
        request: AgentRunRequest,
        options: AgentRunAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        let prompt = match &request {
            AgentRunRequest::Prompt { prompt } => Some(prompt.clone()),
            AgentRunRequest::Retry { .. } => None,
        };
        let run = self
            .inner
            .subagents
            .run(
                agent_id.clone(),
                request,
                RunAgentOptions {
                    signal: options.signal.clone(),
                    summary_policy: None,
                    on_ready: options.on_ready.clone(),
                },
            )
            .await?;
        let completion_profile = profile_name.clone();
        let completion = async move {
            let completion = mirror_agent_run(
                &caller,
                run,
                MirrorAgentRunOptions {
                    profile_name: completion_profile,
                    prompt,
                    suppress_rate_limit_failure_event: options.suppress_rate_limit_failure_event,
                    signal: options.signal,
                    cancel: None,
                },
            )
            .await?;
            Ok(AgentRunCompletion {
                result: completion.summary,
                usage: completion.usage,
            })
        }
        .boxed();
        Ok(AgentRunAttemptHandle {
            agent_id,
            profile_name,
            completion,
        })
    }

    fn publish_suspended(&self, caller_agent_id: &str, agent_id: &str, reason: &str) {
        let Some(caller) = self.inner.lifecycle.get(caller_agent_id) else {
            return;
        };
        if let Ok(bus) = caller.get(EVENT_BUS_SERVICE_ID) {
            bus.publish(DomainEvent::new(
                "subagent.suspended",
                serde_json::Map::from_iter([
                    ("subagentId".into(), Value::String(agent_id.into())),
                    ("reason".into(), Value::String(reason.into())),
                ]),
            ));
        }
    }
}

struct SwarmLauncher {
    service: SessionSwarmService,
    caller_agent_id: String,
}

#[async_trait]
impl AgentRunBatchLauncher<Value> for SwarmLauncher {
    async fn spawn(
        &self,
        options: AgentSpawnAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        self.service
            .spawn_attempt(&self.caller_agent_id, options)
            .await
    }

    async fn resume(
        &self,
        agent_id: &str,
        options: AgentRunAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        self.service
            .resume_attempt(&self.caller_agent_id, agent_id, options, false)
            .await
    }

    async fn retry(
        &self,
        agent_id: &str,
        options: AgentRunAttemptOptions,
    ) -> Result<AgentRunAttemptHandle, BoxError> {
        self.service
            .resume_attempt(&self.caller_agent_id, agent_id, options, true)
            .await
    }

    fn suspended(&self, event: AgentRunSuspendedEvent<Value>) {
        self.service
            .publish_suspended(&self.caller_agent_id, &event.agent_id, &event.reason);
    }
}

impl SessionSwarmServiceContract for SessionSwarmService {
    fn get_swarm_item(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
    ) -> BoxFuture<'static, Result<Option<String>, BoxError>> {
        let service = self.clone();
        let caller_agent_id = caller_agent_id.to_owned();
        let agent_id = agent_id.to_owned();
        async move {
            let meta = service.agent_meta(&agent_id).await?;
            if !is_subagent_meta(meta.as_ref())
                || subagent_parent_agent_id(meta.as_ref()).as_deref()
                    != Some(caller_agent_id.as_str())
            {
                return Ok(None);
            }
            Ok(subagent_swarm_item(meta.as_ref()))
        }
        .boxed()
    }

    fn run(&self, args: SessionSwarmRunArgs<Value>) -> SessionSwarmFuture {
        let service = self.clone();
        async move {
            let generation = service
                .inner
                .next_generation
                .fetch_add(1, Ordering::Relaxed);
            let controller = AbortController::new();
            service.inner.in_flight.lock().unwrap().insert(
                args.caller_agent_id.clone(),
                InFlight {
                    generation,
                    controller: controller.clone(),
                },
            );
            let mut links = Vec::new();
            let tasks = args
                .tasks
                .into_iter()
                .map(|task| link_task(task, &controller, &mut links))
                .collect();
            let max_concurrency = service
                .inner
                .bootstrap
                .get_env(super::AGENT_SWARM_MAX_CONCURRENCY_ENV)
                .map(|raw| {
                    resolve_swarm_max_concurrency(&std::collections::HashMap::from([(
                        super::AGENT_SWARM_MAX_CONCURRENCY_ENV.into(),
                        raw.to_owned(),
                    )]))
                })
                .transpose()?
                .flatten();
            let launcher = Arc::new(SwarmLauncher {
                service: service.clone(),
                caller_agent_id: args.caller_agent_id.clone(),
            });
            let result =
                AgentRunBatch::new(launcher, tasks, AgentRunBatchOptions { max_concurrency })
                    .run()
                    .await;
            drop(links);
            let mut in_flight = service.inner.in_flight.lock().unwrap();
            if in_flight
                .get(&args.caller_agent_id)
                .is_some_and(|entry| entry.generation == generation)
            {
                in_flight.remove(&args.caller_agent_id);
            }
            result
        }
        .boxed()
    }

    fn cancel(&self, caller_agent_id: &str) {
        if let Some(entry) = self.inner.in_flight.lock().unwrap().get(caller_agent_id) {
            entry.controller.abort(None);
        }
    }
}

fn spawned_meta(options: &AgentRunAttemptOptions, profile_name: &str) -> AgentRunSpawnedMeta {
    AgentRunSpawnedMeta {
        profile_name: profile_name.into(),
        parent_tool_call_id: Some(options.parent_tool_call_id.clone()),
        parent_tool_call_uuid: options.parent_tool_call_uuid.clone(),
        description: Some(options.description.clone()),
        swarm_index: options.swarm_index,
        run_in_background: Some(options.run_in_background),
    }
}

fn link_task(
    task: SessionSwarmTask<Value>,
    controller: &AbortController,
    links: &mut Vec<crate::_base::utils::abort::AbortLink>,
) -> SessionSwarmTask<Value> {
    match task {
        SessionSwarmTask::Spawn(mut base) => {
            if let Some(signal) = &base.signal {
                links.push(link_abort_signal(signal, controller.clone()));
            }
            base.signal = Some(controller.signal());
            SessionSwarmTask::Spawn(base)
        }
        SessionSwarmTask::Resume {
            mut base,
            resume_agent_id,
        } => {
            if let Some(signal) = &base.signal {
                links.push(link_abort_signal(signal, controller.clone()));
            }
            base.signal = Some(controller.signal());
            SessionSwarmTask::Resume {
                base,
                resume_agent_id,
            }
        }
    }
}

pub fn register_session_swarm_service() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_SWARM_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = SessionSwarmService::new(
                (*accessor.get(AGENT_LIFECYCLE_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_SUBAGENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_AGENT_PROFILE_CATALOG_ID)?).clone(),
                (*accessor.get(SESSION_CONTEXT_ID)?).clone(),
                (*accessor.get(SESSION_METADATA_ID)?).clone(),
                (*accessor.get(SESSION_PROCESS_RUNNER_SERVICE_ID)?).clone(),
                (*accessor.get(LOG_SERVICE_ID)?).clone(),
                (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone(),
            );
            let contract: Arc<dyn SessionSwarmServiceContract> = Arc::new(service);
            Ok(SessionSwarmServiceHandle(contract))
        }),
        InstantiationType::Eager,
        "sessionSwarm",
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
        register_session_swarm_service();
        let entries = get_scoped_service_descriptors(LifecycleScope::Session);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_SWARM_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "sessionSwarm"
        }));
        clear_scoped_registry_for_tests();
    }
}
