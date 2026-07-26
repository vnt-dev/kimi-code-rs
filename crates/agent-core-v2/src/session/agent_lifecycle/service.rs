//! Session-scoped agent lifecycle implementation.
//!
//! Original: `session/agentLifecycle/agentLifecycleService.ts`.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use indexmap::IndexMap;
use serde_json::Value;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{INSTANTIATION_SERVICE_ID, ServicesAccessorExt},
            instantiation_service::InstantiationService,
            lifecycle::{Disposable, DisposableHandle, DisposeResult},
            scope::{
                InstantiationType, LifecycleScope, ScopeHandle, ScopeOptions,
                create_scoped_child_handle, register_scoped_service,
            },
            service_collection::ServiceCollection,
        },
        event::{Emitter, Event},
        lifecycle::lifecycle_machine::BoxError,
        utils::abort::abort_error,
    },
    agent::{
        blob::{
            AGENT_BLOB_SERVICE_ID, AgentBlobService, AgentBlobServiceContract,
            AgentBlobServiceHandle,
        },
        context_memory::AGENT_CONTEXT_MEMORY_SERVICE_ID,
        full_compaction::AGENT_FULL_COMPACTION_SERVICE_ID,
        loop_::{AGENT_LOOP_SERVICE_ID, LoopValue},
        permission_mode::{
            AGENT_PERMISSION_MODE_SERVICE_ID, DEFAULT_PERMISSION_MODE_SECTION,
            PERMISSION_MODE_CONFIGURED_MODEL,
        },
        permission_policy::PermissionMode,
        profile::{
            AGENT_PROFILE_SERVICE_ID, BindAgentInput, ProfileBindingSnapshot, ProfileUpdateData,
        },
        scope_context::{AGENT_SCOPE_CONTEXT_ID, AgentScopeContextInput, make_agent_scope_context},
        task::AGENT_TASK_SERVICE_ID,
    },
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        event::event_bus::EVENT_BUS_SERVICE_ID,
        telemetry::{TELEMETRY_SERVICE_ID, TelemetryContextPatch, TelemetryServiceHandle},
    },
    persistence::interface::{
        append_log_store::APPEND_LOG_STORE_SERVICE_ID, blob_store::BLOB_STORE_SERVICE_ID,
    },
    session::{
        interaction::{SESSION_INTERACTION_SERVICE_ID, SessionInteractionServiceHandle},
        mcp::{SESSION_MCP_SERVICE_ID, SessionMcpServiceHandle},
        session_context::{SESSION_CONTEXT_ID, SessionContext},
        session_metadata::{AgentMeta, AgentMetaType, SESSION_METADATA_ID, SessionMetadataHandle},
    },
    wire::{
        contract::{WIRE_SERVICE_ID, WireServiceHandle},
        wire_service::{DomainEventPublisher, WireBlobService, WireService},
    },
};

use super::{
    AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceContract, AgentLifecycleServiceHandle,
    AgentListFilter, AgentScopeHandle, CreateAgentOptions, ForkAgentOptions, MAIN_AGENT_ID,
};

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct LifecycleError(Arc<String>);

impl LifecycleError {
    fn new(message: impl Into<String>) -> Self {
        Self(Arc::new(message.into()))
    }
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LifecycleError {}

impl From<crate::_base::di::errors::DiError> for LifecycleError {
    fn from(error: crate::_base::di::errors::DiError) -> Self {
        Self::new(error.to_string())
    }
}

type Creation = Shared<BoxFuture<'static, Result<ScopeHandle, LifecycleError>>>;

#[derive(Default)]
struct RegistryState {
    handles: IndexMap<String, ScopeHandle>,
    creating: HashMap<String, Creation>,
}

struct AgentLifecycleInner {
    instantiation: InstantiationService,
    context: SessionContext,
    metadata: SessionMetadataHandle,
    bootstrap: BootstrapServiceHandle,
    config: ConfigServiceHandle,
    session_mcp: SessionMcpServiceHandle,
    interaction: SessionInteractionServiceHandle,
    telemetry: TelemetryServiceHandle,
    state: Mutex<RegistryState>,
    interaction_subscriptions: Mutex<HashMap<String, DisposableHandle>>,
    did_create: Emitter<ScopeHandle>,
    did_dispose: Emitter<String>,
}

impl Drop for AgentLifecycleInner {
    fn drop(&mut self) {
        let subscriptions = std::mem::take(self.interaction_subscriptions.get_mut().unwrap());
        for disposable in subscriptions.into_values() {
            let _ = disposable.dispose();
        }
    }
}

#[derive(Clone)]
pub struct AgentLifecycleService {
    inner: Arc<AgentLifecycleInner>,
}

impl AgentLifecycleService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instantiation: InstantiationService,
        context: SessionContext,
        metadata: SessionMetadataHandle,
        bootstrap: BootstrapServiceHandle,
        config: ConfigServiceHandle,
        session_mcp: SessionMcpServiceHandle,
        interaction: SessionInteractionServiceHandle,
        telemetry: TelemetryServiceHandle,
    ) -> Self {
        Self {
            inner: Arc::new(AgentLifecycleInner {
                instantiation,
                context,
                metadata,
                bootstrap,
                config,
                session_mcp,
                interaction,
                telemetry,
                state: Mutex::new(RegistryState::default()),
                interaction_subscriptions: Mutex::new(HashMap::new()),
                did_create: Emitter::new(),
                did_dispose: Emitter::new(),
            }),
        }
    }

    async fn create_inner(
        &self,
        options: CreateAgentOptions,
    ) -> Result<ScopeHandle, LifecycleError> {
        if let Some(agent_id) = options.agent_id.as_deref() {
            let (creation, handle) = {
                let state = self.inner.state.lock().unwrap();
                (
                    state.creating.get(agent_id).cloned(),
                    state.handles.get(agent_id).cloned(),
                )
            };
            if let Some(creation) = creation {
                return creation.await;
            }
            if let Some(handle) = handle {
                return Ok(handle);
            }
        }

        let agent_id = match options.agent_id.clone() {
            Some(agent_id) => agent_id,
            None => self.next_available_agent_id().await?,
        };
        let (creation, should_drive) = {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(creation) = state.creating.get(&agent_id) {
                (creation.clone(), false)
            } else if let Some(handle) = state.handles.get(&agent_id) {
                return Ok(handle.clone());
            } else {
                let lifecycle = self.clone();
                let cleanup = self.clone();
                let id = agent_id.clone();
                let cleanup_id = agent_id.clone();
                let creation = async move {
                    let result = lifecycle.do_create(id, options).await;
                    cleanup
                        .inner
                        .state
                        .lock()
                        .unwrap()
                        .creating
                        .remove(&cleanup_id);
                    result
                }
                .boxed()
                .shared();
                state.creating.insert(agent_id.clone(), creation.clone());
                (creation, true)
            }
        };
        // A JavaScript Promise starts immediately and keeps running even when
        // its first caller stops awaiting it. Drive the shared Rust future in
        // the background to preserve that single-flight/bootstrap behavior
        // and guarantee the creating entry is eventually removed.
        if should_drive {
            let driver = creation.clone();
            tokio::spawn(async move {
                let _ = driver.await;
            });
        }
        creation.await
    }

    async fn next_available_agent_id(&self) -> Result<String, LifecycleError> {
        let mut maximum = None;
        {
            let state = self.inner.state.lock().unwrap();
            for id in state.handles.keys() {
                maximum = maximum.max(agent_suffix(id));
            }
        }
        let persisted = self
            .inner
            .metadata
            .read()
            .await
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        if let Some(agents) = persisted.agents {
            for id in agents.keys() {
                maximum = maximum.max(agent_suffix(id));
            }
        }
        let persisted_next = maximum.map_or(0, |suffix| suffix.saturating_add(1));
        let mut process_next = NEXT_AGENT_ID.load(Ordering::Acquire);
        loop {
            let candidate = persisted_next.max(process_next);
            match NEXT_AGENT_ID.compare_exchange(
                process_next,
                candidate.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(format!("agent-{candidate}")),
                Err(actual) => process_next = actual,
            }
        }
    }

    async fn do_create(
        &self,
        agent_id: String,
        options: CreateAgentOptions,
    ) -> Result<ScopeHandle, LifecycleError> {
        let session_mcp = self.inner.session_mcp.0.clone();
        let mcp_ready = tokio::spawn(async move {
            session_mcp.ensure_mcp_ready(None).await;
        });
        let context = &self.inner.context;
        let agent_homedir = self.inner.bootstrap.agent_homedir(
            &context.workspace_id,
            &context.session_id,
            &agent_id,
        );
        let agent_scope =
            self.inner
                .bootstrap
                .agent_scope(&context.workspace_id, &context.session_id, &agent_id);
        let scope_context = make_agent_scope_context(AgentScopeContextInput {
            agent_id: agent_id.clone(),
            agent_scope: agent_scope.clone(),
        });

        let blobs = self
            .inner
            .instantiation
            .get(BLOB_STORE_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let log = self
            .inner
            .instantiation
            .get(APPEND_LOG_STORE_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let events = self
            .inner
            .instantiation
            .get(EVENT_BUS_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let blob = Arc::new(AgentBlobService::new((*blobs).clone(), &scope_context));
        let wire_blob: Arc<dyn WireBlobService> = blob.clone();
        let event_publisher: Arc<dyn DomainEventPublisher> = Arc::new((*events).clone());
        let wire = Arc::new(WireService::new(
            agent_scope,
            (*log).clone(),
            wire_blob,
            event_publisher,
        ));

        let mut extra = ServiceCollection::new();
        extra.set_instance(AGENT_SCOPE_CONTEXT_ID, Arc::new(scope_context));
        let telemetry = self
            .inner
            .telemetry
            .with_context(&TelemetryContextPatch::from([(
                "agent_id".into(),
                Some(Value::String(agent_id.clone())),
            )]));
        extra.set_instance(TELEMETRY_SERVICE_ID, Arc::new(telemetry));
        let blob_contract: Arc<dyn AgentBlobServiceContract> = blob;
        extra.set_instance(
            AGENT_BLOB_SERVICE_ID,
            Arc::new(AgentBlobServiceHandle(blob_contract)),
        );
        extra.set_instance(
            WIRE_SERVICE_ID,
            Arc::new(WireServiceHandle(Arc::clone(&wire))),
        );

        let handle = create_scoped_child_handle(
            &self.inner.instantiation,
            LifecycleScope::Agent,
            agent_id.clone(),
            ScopeOptions { id: None, extra },
        )
        .map_err(|error| LifecycleError::new(error.to_string()))?;
        self.inner
            .state
            .lock()
            .unwrap()
            .handles
            .insert(agent_id.clone(), handle.clone());

        let startup = async {
            wire.seal()
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
            self.inner
                .metadata
                .register_agent(
                    agent_id.clone(),
                    AgentMeta {
                        homedir: Some(agent_homedir.to_string_lossy().into_owned()),
                        r#type: Some(if agent_id == MAIN_AGENT_ID {
                            AgentMetaType::Main
                        } else {
                            AgentMetaType::Sub
                        }),
                        parent_agent_id: (agent_id != MAIN_AGENT_ID)
                            .then(|| MAIN_AGENT_ID.to_owned()),
                        forked_from: options.forked_from.clone(),
                        labels: options.labels.clone(),
                        swarm_item: None,
                    },
                )
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
            self.subscribe_interaction_bus(&handle)?;
            self.inner.did_create.fire(&handle);
            self.ignite_eager_services(&handle)?;
            mcp_ready
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
            wire.restore()
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
            self.bind_bootstrap(&handle, &options).await?;
            Ok(handle.clone())
        }
        .await;

        if let Err(error) = startup {
            let removed = self
                .inner
                .state
                .lock()
                .unwrap()
                .handles
                .get(&agent_id)
                .is_some_and(|candidate| candidate.id() == handle.id());
            if removed {
                self.inner
                    .state
                    .lock()
                    .unwrap()
                    .handles
                    .shift_remove(&agent_id);
            }
            let _ = handle.dispose();
            self.remove_interaction_subscription(&agent_id);
            self.inner.did_dispose.fire(&agent_id);
            return Err(error);
        }
        startup
    }

    fn subscribe_interaction_bus(&self, handle: &ScopeHandle) -> Result<(), LifecycleError> {
        let agent_id = handle.id().to_owned();
        let mut subscriptions = self.inner.interaction_subscriptions.lock().unwrap();
        if subscriptions.contains_key(&agent_id) {
            return Ok(());
        }
        let events = handle
            .get(EVENT_BUS_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let interaction = self.inner.interaction.0.clone();
        let disposable = events.subscribe_type(
            "turn.ended",
            Arc::new(move |event| {
                let Some(turn_id) = event.fields.get("turnId").and_then(Value::as_f64) else {
                    return;
                };
                let interaction = interaction.clone();
                tokio::spawn(async move {
                    interaction.cancel_pending_for_turn(turn_id).await;
                });
            }),
        );
        subscriptions.insert(agent_id, disposable);
        Ok(())
    }

    fn remove_interaction_subscription(&self, agent_id: &str) {
        if let Some(disposable) = self
            .inner
            .interaction_subscriptions
            .lock()
            .unwrap()
            .remove(agent_id)
        {
            let _ = disposable.dispose();
        }
    }

    fn ignite_eager_services(&self, handle: &ScopeHandle) -> Result<(), LifecycleError> {
        // The order is intentionally the same as the TypeScript source.
        use crate::agent::{
            activity_view::AGENT_ACTIVITY_VIEW_ID,
            context_injector::AGENT_CONTEXT_INJECTOR_SERVICE_ID,
            external_hooks::AGENT_EXTERNAL_HOOKS_SERVICE_ID,
            goal::AGENT_GOAL_SERVICE_ID,
            loop_::AGENT_LOOP_CONTINUATION_SERVICE_ID,
            mcp::AGENT_MCP_SERVICE_ID,
            media::{AGENT_MEDIA_TOOLS_REGISTRAR_ID, IMAGE_CONFIG_BRIDGE_ID},
            plan::AGENT_PLAN_SERVICE_ID,
            plugin::AGENT_PLUGIN_SERVICE_ID,
            step_retry::AGENT_STEP_RETRY_SERVICE_ID,
            tool_dedupe::AGENT_TOOL_DEDUPE_SERVICE_ID,
            tool_registry::AGENT_BUILTIN_TOOLS_REGISTRAR_ID,
            tool_select::{
                AGENT_TOOL_SELECT_ANNOUNCEMENTS_SERVICE_ID, AGENT_TOOL_SELECT_SERVICE_ID,
            },
            user_tool::AGENT_USER_TOOL_SERVICE_ID,
        };

        handle.get(AGENT_BUILTIN_TOOLS_REGISTRAR_ID)?;
        handle.get(AGENT_MEDIA_TOOLS_REGISTRAR_ID)?;
        handle.get(IMAGE_CONFIG_BRIDGE_ID)?;
        handle.get(AGENT_TOOL_DEDUPE_SERVICE_ID)?;
        handle.get(AGENT_EXTERNAL_HOOKS_SERVICE_ID)?;
        handle.get(AGENT_MCP_SERVICE_ID)?;
        handle.get(AGENT_PLUGIN_SERVICE_ID)?;
        handle.get(AGENT_TOOL_SELECT_SERVICE_ID)?;
        handle.get(AGENT_TOOL_SELECT_ANNOUNCEMENTS_SERVICE_ID)?;
        handle.get(AGENT_STEP_RETRY_SERVICE_ID)?;
        handle.get(AGENT_LOOP_CONTINUATION_SERVICE_ID)?;
        handle.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?;
        handle.get(AGENT_CONTEXT_INJECTOR_SERVICE_ID)?;
        handle.get(AGENT_GOAL_SERVICE_ID)?;
        handle.get(AGENT_PLAN_SERVICE_ID)?;
        handle.get(AGENT_TASK_SERVICE_ID)?;
        handle.get(AGENT_USER_TOOL_SERVICE_ID)?;
        handle.get(AGENT_FULL_COMPACTION_SERVICE_ID)?;
        // The activity view exists for its event subscriptions. Nothing
        // injects this delayed service directly, so resolve it before the
        // first turn just like the TypeScript lifecycle does.
        handle.get(AGENT_ACTIVITY_VIEW_ID)?;
        Ok(())
    }

    async fn bind_bootstrap(
        &self,
        handle: &ScopeHandle,
        options: &CreateAgentOptions,
    ) -> Result<(), LifecycleError> {
        if let Some(binding) = options.binding.clone() {
            handle
                .get(AGENT_PROFILE_SERVICE_ID)
                .map_err(|error| LifecycleError::new(error.to_string()))?
                .bind(binding)
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
        }
        let permission_mode = self
            .inner
            .config
            .get(DEFAULT_PERMISSION_MODE_SECTION)
            .map(serde_json::from_value::<PermissionMode>)
            .transpose()
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let wire = handle
            .get(WIRE_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        if let Some(mode) = permission_mode
            && !wire.get_model(&PERMISSION_MODE_CONFIGURED_MODEL)
        {
            handle
                .get(AGENT_PERMISSION_MODE_SERVICE_ID)
                .map_err(|error| LifecycleError::new(error.to_string()))?
                .set_mode(mode)
                .map_err(|error| LifecycleError::new(error.to_string()))?;
        }
        Ok(())
    }

    async fn fork_inner(
        &self,
        source_agent_id: String,
        options: ForkAgentOptions,
    ) -> Result<ScopeHandle, LifecycleError> {
        let source = self.get(&source_agent_id).ok_or_else(|| {
            LifecycleError::new(format!("Source agent \"{source_agent_id}\" does not exist"))
        })?;
        if let Some(agent_id) = options.agent_id.as_deref()
            && self.get(agent_id).is_some()
        {
            return Err(LifecycleError::new(format!(
                "Agent \"{agent_id}\" already exists"
            )));
        }
        let child = self
            .create_inner(CreateAgentOptions {
                agent_id: options.agent_id.clone(),
                forked_from: Some(source_agent_id),
                ..CreateAgentOptions::default()
            })
            .await?;
        let source_profile = source
            .get(AGENT_PROFILE_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let source_data = source_profile
            .data()
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let child_profile = child
            .get(AGENT_PROFILE_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let binding = options.binding.unwrap_or_default();
        if let Some(profile) = binding.profile {
            child_profile
                .bind(BindAgentInput {
                    profile,
                    model: binding.model.or(source_data.config.model_alias.clone()),
                    thinking: binding
                        .thinking
                        .or(Some(source_data.config.thinking_level.clone())),
                    strict_thinking: None,
                    cwd: binding.cwd.or(Some(source_data.config.cwd.clone())),
                })
                .await
                .map_err(|error| LifecycleError::new(error.to_string()))?;
        } else {
            child_profile
                .apply_binding_snapshot(ProfileBindingSnapshot {
                    cwd: source_data.config.cwd,
                    model_alias: source_data.config.model_alias,
                    profile_name: source_data.config.profile_name,
                    thinking_level: source_data.config.thinking_level,
                    system_prompt: source_data.config.system_prompt,
                    active_tool_names: source_data.active_tool_names,
                    disallowed_tools: source_data.disallowed_tools,
                    subagents: source_data.subagents,
                })
                .map_err(|error| LifecycleError::new(error.to_string()))?;
            if let Some(model) = binding.model {
                child_profile
                    .set_model(model)
                    .await
                    .map_err(|error| LifecycleError::new(error.to_string()))?;
            }
            if let Some(thinking) = binding.thinking {
                child_profile
                    .set_thinking(thinking)
                    .map_err(|error| LifecycleError::new(error.to_string()))?;
            }
            if let Some(cwd) = binding.cwd {
                child_profile
                    .update(ProfileUpdateData {
                        cwd: Some(cwd),
                        ..ProfileUpdateData::default()
                    })
                    .map_err(|error| LifecycleError::new(error.to_string()))?;
            }
        }
        let messages = source
            .get(AGENT_CONTEXT_MEMORY_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?
            .get();
        if !messages.is_empty() {
            child
                .get(AGENT_CONTEXT_MEMORY_SERVICE_ID)
                .map_err(|error| LifecycleError::new(error.to_string()))?
                .append(messages)
                .map_err(|error| LifecycleError::new(error.to_string()))?;
        }
        Ok(child)
    }

    async fn remove_inner(&self, agent_id: String) -> Result<(), LifecycleError> {
        let handle = self
            .inner
            .state
            .lock()
            .unwrap()
            .handles
            .shift_remove(&agent_id);
        let Some(handle) = handle else {
            return Ok(());
        };
        handle
            .get(AGENT_TASK_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?
            .stop_all_on_exit("Session closed")
            .await
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let loop_service = handle
            .get(AGENT_LOOP_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        let compaction = handle
            .get(AGENT_FULL_COMPACTION_SERVICE_ID)
            .map_err(|error| LifecycleError::new(error.to_string()))?
            .compacting();
        let reason = Arc::new(abort_error(Some("Agent removed")));
        for turn_id in loop_service.status().pending_turn_ids {
            loop_service.cancel(Some(turn_id), Some(LoopValue::Error(reason.clone())));
        }
        loop_service.cancel(None, Some(LoopValue::Error(reason.clone())));
        if let Some(task) = compaction.as_ref()
            && !task.abort_controller.signal().aborted()
        {
            task.abort_controller.abort(Some((*reason).clone()));
        }
        let compaction_settled = async move {
            if let Some(task) = compaction {
                let _ = task.promise.await;
            }
        };
        tokio::join!(loop_service.settled(), compaction_settled);
        handle
            .dispose()
            .map_err(|error| LifecycleError::new(error.to_string()))?;
        self.remove_interaction_subscription(&agent_id);
        self.inner.did_dispose.fire(&agent_id);
        Ok(())
    }
}

impl AgentLifecycleServiceContract for AgentLifecycleService {
    fn on_did_create(&self) -> Event<AgentScopeHandle> {
        self.inner.did_create.event()
    }

    fn on_did_dispose(&self) -> Event<String> {
        self.inner.did_dispose.event()
    }

    fn create(
        &self,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
        let lifecycle = self.clone();
        Box::pin(async move {
            lifecycle
                .create_inner(options)
                .await
                .map_err(|error| Box::new(error) as BoxError)
        })
    }

    fn fork(
        &self,
        source_agent_id: String,
        options: ForkAgentOptions,
    ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
        let lifecycle = self.clone();
        Box::pin(async move {
            lifecycle
                .fork_inner(source_agent_id, options)
                .await
                .map_err(|error| Box::new(error) as BoxError)
        })
    }

    fn get(&self, agent_id: &str) -> Option<AgentScopeHandle> {
        self.inner
            .state
            .lock()
            .unwrap()
            .handles
            .get(agent_id)
            .cloned()
    }

    fn list(&self, filter: Option<&AgentListFilter>) -> Vec<AgentScopeHandle> {
        self.inner
            .state
            .lock()
            .unwrap()
            .handles
            .iter()
            .filter(|(id, _)| {
                filter
                    .and_then(|filter| filter.prefix.as_deref())
                    .is_none_or(|prefix| id.starts_with(prefix))
            })
            .map(|(_, handle)| handle.clone())
            .collect()
    }

    fn broadcast_permission_mode(&self, mode: PermissionMode) -> Result<(), BoxError> {
        for handle in self.list(None) {
            handle
                .get(AGENT_PERMISSION_MODE_SERVICE_ID)
                .map_err(|error| Box::new(error) as BoxError)?
                .set_mode(mode)
                .map_err(|error| Box::new(error) as BoxError)?;
        }
        Ok(())
    }

    fn remove(&self, agent_id: String) -> BoxFuture<'static, Result<(), BoxError>> {
        let lifecycle = self.clone();
        Box::pin(async move {
            lifecycle
                .remove_inner(agent_id)
                .await
                .map_err(|error| Box::new(error) as BoxError)
        })
    }
}

impl Disposable for AgentLifecycleService {
    fn dispose(&self) -> DisposeResult {
        let subscriptions =
            std::mem::take(&mut *self.inner.interaction_subscriptions.lock().unwrap());
        for disposable in subscriptions.into_values() {
            disposable.dispose()?;
        }
        self.inner.did_dispose.dispose()?;
        self.inner.did_create.dispose()
    }
}

pub fn register_agent_lifecycle_service() {
    register_scoped_service(
        LifecycleScope::Session,
        AGENT_LIFECYCLE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let instantiation = (*accessor.get(INSTANTIATION_SERVICE_ID)?).clone();
            let context = (*accessor.get(SESSION_CONTEXT_ID)?).clone();
            let metadata = (*accessor.get(SESSION_METADATA_ID)?).clone();
            let bootstrap = (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone();
            let config = (*accessor.get(CONFIG_SERVICE_ID)?).clone();
            let session_mcp = (*accessor.get(SESSION_MCP_SERVICE_ID)?).clone();
            let interaction = (*accessor.get(SESSION_INTERACTION_SERVICE_ID)?).clone();
            let telemetry = (*accessor.get(TELEMETRY_SERVICE_ID)?).clone();
            let service = AgentLifecycleService::new(
                instantiation,
                context,
                metadata,
                bootstrap,
                config,
                session_mcp,
                interaction,
                telemetry,
            );
            let contract: Arc<dyn AgentLifecycleServiceContract> = Arc::new(service);
            Ok(AgentLifecycleServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "agentLifecycle",
    );
}

fn agent_suffix(agent_id: &str) -> Option<u64> {
    agent_id
        .strip_prefix("agent-")
        .filter(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|suffix| suffix.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_id_suffix_parser_matches_source_pattern() {
        assert_eq!(agent_suffix("agent-0"), Some(0));
        assert_eq!(agent_suffix("agent-0042"), Some(42));
        assert_eq!(agent_suffix("main"), None);
        assert_eq!(agent_suffix("agent-"), None);
        assert_eq!(agent_suffix("agent--1"), None);
        assert_eq!(agent_suffix("agent-1-child"), None);
    }

    #[test]
    fn registration_is_eager_session_scoped_in_lifecycle_domain() {
        register_agent_lifecycle_service();
        let descriptor =
            crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::Session)
                .into_iter()
                .find(|entry| entry.id.to_string() == AGENT_LIFECYCLE_SERVICE_ID.to_string())
                .expect("agent lifecycle service is registered");
        assert!(!descriptor.descriptor.supports_delayed_instantiation);
        assert_eq!(descriptor.domain, "agentLifecycle");
    }
}
