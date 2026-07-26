//! Process-wide session lifecycle implementation.
//!
//! Original:
//! `packages/agent-core-v2/src/app/sessionLifecycle/sessionLifecycleService.ts`.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use futures_util::{
    FutureExt, TryStreamExt,
    future::{BoxFuture, Shared},
};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value, json};
use ulid::Ulid;
use uuid::Uuid;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::{INSTANTIATION_SERVICE_ID, ServicesAccessorExt},
            instantiation_service::InstantiationService,
            lifecycle::{Disposable, DisposeResult},
            scope::{
                InstantiationType, LifecycleScope, ScopeOptions, create_scoped_child_handle,
                register_scoped_service,
            },
        },
        errors::errors::Error2,
        event::{Emitter, Event},
    },
    agent::plan::{AGENT_PLAN_SERVICE_ID, config_section::DEFAULT_PLAN_MODE_SECTION},
    app::{
        bootstrap::{BOOTSTRAP_SERVICE_ID, BootstrapServiceHandle},
        config::{CONFIG_SERVICE_ID, ConfigServiceHandle},
        cron::{
            CRON_SESSION_TAG, CRON_TASK_PERSISTENCE_SERVICE_ID, CronTaskPersistenceHandle,
            CronTaskQuery,
        },
        event::{EVENT_SERVICE_ID, EventServiceHandle, GlobalDomainEvent},
        session_index::{
            CHILD_SESSION_KIND, CHILD_SESSION_KIND_KEY, PARENT_SESSION_ID_KEY,
            SESSION_INDEX_SERVICE_ID, SessionIndexHandle,
        },
        telemetry::{
            SessionLoadFailedEvent, SessionStartedEvent, TELEMETRY_SERVICE_ID,
            TelemetryContextPatch, TelemetryServiceEventExt, TelemetryServiceHandle,
        },
        workspace_local_config::{
            WORKSPACE_LOCAL_CONFIG_SERVICE_ID, WorkspaceLocalConfigServiceHandle,
        },
        workspace_registry::{
            WORKSPACE_NOT_FOUND, WORKSPACE_REGISTRY_SERVICE_ID, WorkspaceRegistryHandle,
        },
    },
    os::interface::{
        host_environment::{HOST_ENVIRONMENT_SERVICE_ID, HostEnvironmentHandle},
        host_file_system::{
            HOST_FILE_SYSTEM_SERVICE_ID, HostDirEntry, HostFileSystemServiceHandle,
        },
        host_fs_errors::OS_FS_NOT_FOUND,
    },
    persistence::interface::{
        append_log_store::{APPEND_LOG_STORE_SERVICE_ID, AppendLogOptions, AppendLogStoreHandle},
        atomic_document_store::{ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle},
    },
    session::{
        agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, CreateAgentOptions, MAIN_AGENT_ID, ensure_main_agent,
            labels_from_agent_meta,
        },
        agent_profile_catalog::SESSION_AGENT_PROFILE_CATALOG_ID,
        cron::SESSION_CRON_SERVICE_ID,
        external_hooks::SESSION_EXTERNAL_HOOKS_SERVICE_ID,
        mcp::SESSION_MCP_SERVICE_ID,
        session_context::{
            SESSION_CONTEXT_ID, SessionContextInput, make_session_context, session_context_seed,
        },
        session_metadata::{SESSION_METADATA_ID, SessionMeta, SessionMetaPatch},
        skill_catalog::SESSION_SKILL_CATALOG_ID,
        tool_policy::SESSION_TOOL_POLICY_ID,
        workspace_context::SESSION_WORKSPACE_CONTEXT_ID,
    },
    wire::{
        contract::WIRE_SERVICE_ID,
        record::{AGENT_WIRE_RECORD_KEY, WireRecord, create_wire_metadata_record},
    },
};

use super::{
    CreateChildSessionOptions, CreateSessionOptions, ForkSessionOptions,
    SESSION_LIFECYCLE_SERVICE_ID, SessionArchivedEvent, SessionCloseReason, SessionClosedEvent,
    SessionCreateSource, SessionCreatedEvent, SessionForkedEvent, SessionLifecycleError,
    SessionLifecycleHooks, SessionLifecycleServiceContract, SessionLifecycleServiceHandle,
    SessionScopeHandle, SessionWillCloseEvent,
};

const SESSION_NOT_FOUND: &str = "session.not_found";
const SESSION_ALREADY_EXISTS: &str = "session.already_exists";
const SESSION_INDEX_KEY: &str = "session_index.jsonl";
const SESSION_STATE_KEY: &str = "state.json";

type ResumeResult = Result<Option<SessionScopeHandle>, SessionLifecycleError>;
type ResumeFuture = Shared<BoxFuture<'static, ResumeResult>>;

struct ResumeEntry {
    generation: u64,
    future: ResumeFuture,
}

struct SessionLifecycleInner {
    instantiation: InstantiationService,
    bootstrap: BootstrapServiceHandle,
    config: ConfigServiceHandle,
    host_env: HostEnvironmentHandle,
    index: SessionIndexHandle,
    append_log_store: AppendLogStoreHandle,
    docs: AtomicDocumentStoreHandle,
    host_fs: HostFileSystemServiceHandle,
    cron_store: CronTaskPersistenceHandle,
    workspace_registry: WorkspaceRegistryHandle,
    workspace_local_config: WorkspaceLocalConfigServiceHandle,
    event: EventServiceHandle,
    telemetry: TelemetryServiceHandle,
    sessions: Mutex<IndexMap<String, SessionScopeHandle>>,
    resuming: Mutex<HashMap<String, ResumeEntry>>,
    next_resume_generation: AtomicU64,
    did_create: Emitter<SessionCreatedEvent>,
    did_close: Emitter<SessionClosedEvent>,
    did_archive: Emitter<SessionArchivedEvent>,
    did_fork: Emitter<SessionForkedEvent>,
    hooks: SessionLifecycleHooks,
}

#[derive(Clone)]
pub struct SessionLifecycleService {
    inner: Arc<SessionLifecycleInner>,
}

impl SessionLifecycleService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instantiation: InstantiationService,
        bootstrap: BootstrapServiceHandle,
        config: ConfigServiceHandle,
        host_env: HostEnvironmentHandle,
        index: SessionIndexHandle,
        append_log_store: AppendLogStoreHandle,
        docs: AtomicDocumentStoreHandle,
        host_fs: HostFileSystemServiceHandle,
        cron_store: CronTaskPersistenceHandle,
        workspace_registry: WorkspaceRegistryHandle,
        workspace_local_config: WorkspaceLocalConfigServiceHandle,
        event: EventServiceHandle,
        telemetry: TelemetryServiceHandle,
    ) -> Self {
        Self {
            inner: Arc::new(SessionLifecycleInner {
                instantiation,
                bootstrap,
                config,
                host_env,
                index,
                append_log_store,
                docs,
                host_fs,
                cron_store,
                workspace_registry,
                workspace_local_config,
                event,
                telemetry,
                sessions: Mutex::new(IndexMap::new()),
                resuming: Mutex::new(HashMap::new()),
                next_resume_generation: AtomicU64::new(1),
                did_create: Emitter::new(),
                did_close: Emitter::new(),
                did_archive: Emitter::new(),
                did_fork: Emitter::new(),
                hooks: SessionLifecycleHooks::default(),
            }),
        }
    }

    async fn materialize_session(&self, options: MaterializeSessionOptions) -> ResumeResult {
        let workspace = self
            .inner
            .workspace_registry
            .create_or_touch(&options.work_dir, None)
            .await
            .map_err(boxed_error)?;
        let workspace_id = options.workspace_id.unwrap_or(workspace.id);
        let session_scope = self
            .inner
            .bootstrap
            .session_scope(&workspace_id, &options.session_id);
        let session_dir = self
            .inner
            .bootstrap
            .session_dir(&workspace_id, &options.session_id);
        let context = make_session_context(SessionContextInput {
            session_id: options.session_id.clone(),
            workspace_id,
            session_dir: session_dir.to_string_lossy().into_owned(),
            session_scope: session_scope.clone(),
            cwd: options.work_dir.clone(),
            meta_scope: Some(session_scope),
        });

        let local_dirs = self
            .inner
            .workspace_local_config
            .read_additional_dirs(&options.work_dir)
            .await
            .map_err(boxed_error)?;
        let caller_dirs = self
            .inner
            .workspace_local_config
            .resolve_additional_dirs(
                &options.work_dir,
                options.additional_dirs.as_deref().unwrap_or_default(),
            )
            .await
            .map_err(boxed_error)?;
        let additional_dirs = local_dirs
            .additional_dirs
            .into_iter()
            .chain(caller_dirs)
            .collect::<Vec<_>>();

        self.inner.host_env.ready().await.map_err(shared_error)?;
        let handle = create_scoped_child_handle(
            &self.inner.instantiation,
            LifecycleScope::Session,
            &options.session_id,
            ScopeOptions {
                id: None,
                extra: session_context_seed(context),
            },
        )
        .map_err(shared_error)?;

        if !additional_dirs.is_empty()
            && let Err(error) = handle
                .get(SESSION_WORKSPACE_CONTEXT_ID)
                .map_err(shared_error)
                .and_then(|workspace| {
                    workspace
                        .set_additional_dirs(&additional_dirs)
                        .map_err(shared_error)
                })
        {
            let _ = handle.dispose();
            return Err(error);
        }

        let initialized = async {
            handle
                .get(SESSION_METADATA_ID)
                .map_err(shared_error)?
                .ready()
                .await
                .map_err(boxed_error)?;
            handle
                .get(SESSION_TOOL_POLICY_ID)
                .map_err(shared_error)?
                .ready()
                .await
                .map_err(boxed_error)?;

            let skills = handle.get(SESSION_SKILL_CATALOG_ID).map_err(shared_error)?;
            tokio::spawn(async move {
                let _ = skills.ready().await;
            });

            handle
                .get(SESSION_AGENT_PROFILE_CATALOG_ID)
                .map_err(shared_error)?
                .ready()
                .await
                .map_err(boxed_error)?;
            handle
                .get(SESSION_MCP_SERVICE_ID)
                .map_err(shared_error)?
                .0
                .ensure_mcp_ready(options.mcp_servers)
                .await;
            handle
                .get(SESSION_EXTERNAL_HOOKS_SERVICE_ID)
                .map_err(shared_error)?;
            handle.get(SESSION_CRON_SERVICE_ID).map_err(shared_error)?;
            Ok::<(), SessionLifecycleError>(())
        }
        .await;

        if let Err(error) = initialized {
            let _ = handle.dispose();
            return Err(error);
        }
        self.inner
            .sessions
            .lock()
            .unwrap()
            .insert(options.session_id, handle.clone());
        Ok(Some(handle))
    }

    async fn append_session_index_entry(
        &self,
        session_id: &str,
        work_dir: &str,
        workspace_id: &str,
    ) -> Result<(), SessionLifecycleError> {
        let record = LegacySessionIndexEntry {
            session_id,
            session_dir: self
                .inner
                .bootstrap
                .session_dir(workspace_id, session_id)
                .to_string_lossy()
                .into_owned(),
            work_dir,
        };
        self.inner
            .append_log_store
            .append("", SESSION_INDEX_KEY, &record, AppendLogOptions::default())
            .map_err(shared_error)?;
        self.inner
            .append_log_store
            .flush()
            .await
            .map_err(shared_error)
    }

    async fn announce_created(
        &self,
        mut event: SessionCreatedEvent,
    ) -> Result<(), SessionLifecycleError> {
        self.inner
            .hooks
            .on_did_create_session
            .run(&mut event, None)
            .await
            .map_err(Arc::from)?;
        self.inner.did_create.fire(&event);
        self.inner
            .telemetry
            .set_context(&TelemetryContextPatch::from([(
                "sessionId".into(),
                Some(Value::String(event.session_id.clone())),
            )]));
        let _ = self.inner.telemetry.track_event(&SessionStartedEvent {
            resumed: event.source == SessionCreateSource::Resume,
        });
        Ok(())
    }

    async fn announce_will_close(
        &self,
        mut event: SessionWillCloseEvent,
    ) -> Result<(), SessionLifecycleError> {
        self.inner
            .hooks
            .on_will_close_session
            .run(&mut event, None)
            .await
            .map_err(Arc::from)
    }

    async fn do_resume(&self, session_id: &str) -> ResumeResult {
        if let Some(handle) = self.inner.sessions.lock().unwrap().get(session_id).cloned() {
            return Ok(Some(handle));
        }
        let Some(summary) = self
            .inner
            .index
            .get(session_id)
            .await
            .map_err(boxed_error)?
        else {
            return Ok(None);
        };
        let work_dir = match summary.cwd {
            Some(cwd) => Some(cwd),
            None => self
                .inner
                .workspace_registry
                .get(&summary.workspace_id)
                .await
                .map_err(boxed_error)?
                .map(|workspace| workspace.root),
        };
        let Some(work_dir) = work_dir else {
            return Ok(None);
        };
        let handle = self
            .materialize_session(MaterializeSessionOptions {
                session_id: session_id.into(),
                work_dir,
                additional_dirs: None,
                mcp_servers: None,
                workspace_id: Some(summary.workspace_id),
            })
            .await?
            .expect("materialize always returns a handle");
        let agents = handle
            .get(AGENT_LIFECYCLE_SERVICE_ID)
            .map_err(shared_error)?;
        if agents.get(MAIN_AGENT_ID).is_none() {
            agents
                .create(CreateAgentOptions {
                    agent_id: Some(MAIN_AGENT_ID.into()),
                    ..CreateAgentOptions::default()
                })
                .await
                .map_err(Arc::from)?;
        }
        self.announce_created(SessionCreatedEvent {
            session_id: session_id.into(),
            handle: handle.clone(),
            source: SessionCreateSource::Resume,
        })
        .await?;
        Ok(Some(handle))
    }

    async fn drain_agents(&self, handle: &SessionScopeHandle) -> Result<(), SessionLifecycleError> {
        let agents = handle
            .get(AGENT_LIFECYCLE_SERVICE_ID)
            .map_err(shared_error)?;
        for agent in agents.list(None) {
            agents.remove(agent.id().into()).await.map_err(Arc::from)?;
        }
        Ok(())
    }

    async fn resolve_source_title(
        &self,
        source_id: &str,
    ) -> Result<Option<String>, SessionLifecycleError> {
        let live = { self.inner.sessions.lock().unwrap().get(source_id).cloned() };
        if let Some(live) = live {
            return Ok(live
                .get(SESSION_METADATA_ID)
                .map_err(shared_error)?
                .read()
                .await
                .map_err(boxed_error)?
                .title);
        }
        Ok(self
            .inner
            .index
            .get(source_id)
            .await
            .map_err(boxed_error)?
            .and_then(|summary| summary.title))
    }

    async fn copy_agent_wire(
        &self,
        args: CopyAgentWireOptions<'_>,
    ) -> Result<(), SessionLifecycleError> {
        if let Some(source_handle) = args.source_handle
            && let Some(agent) = source_handle
                .get(AGENT_LIFECYCLE_SERVICE_ID)
                .map_err(shared_error)?
                .get(args.agent_id)
        {
            agent
                .get(WIRE_SERVICE_ID)
                .map_err(shared_error)?
                .flush()
                .await
                .map_err(shared_error)?;
        }

        let source_scope = self.inner.bootstrap.agent_scope(
            args.source_workspace_id,
            args.source_session_id,
            args.agent_id,
        );
        let mut records = self
            .inner
            .append_log_store
            .read::<WireRecord>(&source_scope, AGENT_WIRE_RECORD_KEY)
            .try_collect::<Vec<_>>()
            .await
            .map_err(shared_error)?;
        if records.is_empty() {
            records.push(create_wire_metadata_record().into_wire_record());
        } else if records[0].get("type").and_then(Value::as_str) != Some("metadata") {
            records.insert(0, create_wire_metadata_record().into_wire_record());
        }
        records.push(forked_record());
        let target_scope = self.inner.bootstrap.agent_scope(
            args.target_workspace_id,
            args.target_session_id,
            args.agent_id,
        );
        self.inner
            .append_log_store
            .rewrite(&target_scope, AGENT_WIRE_RECORD_KEY, &records)
            .await
            .map_err(shared_error)
    }

    async fn copy_session_files(
        &self,
        source_dir: &Path,
        target_dir: &Path,
    ) -> Result<(), SessionLifecycleError> {
        let entries = match self.inner.host_fs.read_dir(source_dir).await {
            Ok(entries) => entries,
            Err(error) if error.code() == OS_FS_NOT_FOUND => return Ok(()),
            Err(error) => return Err(shared_error(error)),
        };
        self.copy_session_dir_entries(source_dir, target_dir, entries, "")
            .await
    }

    fn copy_session_dir_entries<'a>(
        &'a self,
        source_dir: &'a Path,
        target_dir: &'a Path,
        entries: Vec<HostDirEntry>,
        relative_base: &'a str,
    ) -> BoxFuture<'a, Result<(), SessionLifecycleError>> {
        Box::pin(async move {
            for entry in entries {
                let relative = if relative_base.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{relative_base}/{}", entry.name)
                };
                if relative == SESSION_STATE_KEY
                    || relative == "logs"
                    || entry.name == AGENT_WIRE_RECORD_KEY
                    || entry.is_symbolic_link
                {
                    continue;
                }
                let source_path = source_dir.join(&entry.name);
                let target_path = target_dir.join(&entry.name);
                if entry.is_directory {
                    let children = match self.inner.host_fs.read_dir(&source_path).await {
                        Ok(children) => children,
                        Err(error) if error.code() == OS_FS_NOT_FOUND => continue,
                        Err(error) => return Err(shared_error(error)),
                    };
                    self.inner
                        .host_fs
                        .create_dir(&target_path, true)
                        .await
                        .map_err(shared_error)?;
                    self.copy_session_dir_entries(&source_path, &target_path, children, &relative)
                        .await?;
                } else if entry.is_file {
                    let data = self
                        .inner
                        .host_fs
                        .read_bytes(&source_path, None)
                        .await
                        .map_err(shared_error)?;
                    self.inner
                        .host_fs
                        .create_dir(target_dir, true)
                        .await
                        .map_err(shared_error)?;
                    self.inner
                        .host_fs
                        .write_bytes(&target_path, &data)
                        .await
                        .map_err(shared_error)?;
                }
            }
            Ok(())
        })
    }

    async fn duplicate_cron_tasks(
        &self,
        workspace_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), SessionLifecycleError> {
        let tasks = self
            .inner
            .cron_store
            .list(CronTaskQuery {
                workspace_id: workspace_id.into(),
            })
            .await
            .map_err(boxed_error)?;
        for task in tasks {
            if task
                .tags
                .as_ref()
                .and_then(|tags| tags.get(CRON_SESSION_TAG))
                .map(String::as_str)
                != Some(source_id)
            {
                continue;
            }
            let mut clone = task;
            clone.id = Ulid::new().to_string();
            clone
                .tags
                .get_or_insert_default()
                .insert(CRON_SESSION_TAG.into(), target_id.into());
            self.inner
                .cron_store
                .save(workspace_id, &clone)
                .await
                .map_err(boxed_error)?;
        }
        Ok(())
    }

    async fn read_meta_from_disk(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionMeta>, SessionLifecycleError> {
        self.inner
            .docs
            .get(
                &self.inner.bootstrap.session_scope(workspace_id, session_id),
                SESSION_STATE_KEY,
            )
            .await
            .map_err(shared_error)
    }
}

#[async_trait]
impl SessionLifecycleServiceContract for SessionLifecycleService {
    fn on_did_create_session(&self) -> Event<SessionCreatedEvent> {
        self.inner.did_create.event()
    }

    fn on_did_close_session(&self) -> Event<SessionClosedEvent> {
        self.inner.did_close.event()
    }

    fn on_did_archive_session(&self) -> Event<SessionArchivedEvent> {
        self.inner.did_archive.event()
    }

    fn on_did_fork_session(&self) -> Event<SessionForkedEvent> {
        self.inner.did_fork.event()
    }

    fn hooks(&self) -> &SessionLifecycleHooks {
        &self.inner.hooks
    }

    async fn create(
        &self,
        options: CreateSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError> {
        let session_id = options.session_id.unwrap_or_else(create_session_id);
        let handle = self
            .materialize_session(MaterializeSessionOptions {
                session_id: session_id.clone(),
                work_dir: options.work_dir.clone(),
                additional_dirs: options.additional_dirs,
                mcp_servers: options.mcp_servers,
                workspace_id: None,
            })
            .await?
            .expect("materialize always returns a handle");
        let initialized = async {
            let main = if let Some(binding) = options.main_agent_binding {
                Some(
                    handle
                        .get(AGENT_LIFECYCLE_SERVICE_ID)
                        .map_err(shared_error)?
                        .create(CreateAgentOptions {
                            agent_id: Some(MAIN_AGENT_ID.into()),
                            binding: Some(binding),
                            ..CreateAgentOptions::default()
                        })
                        .await
                        .map_err(Arc::from)?,
                )
            } else {
                None
            };
            if self.inner.config.get(DEFAULT_PLAN_MODE_SECTION) == Some(Value::Bool(true)) {
                let plan_agent = match main {
                    Some(main) => main,
                    None => ensure_main_agent(&handle, None).await.map_err(Arc::from)?,
                };
                plan_agent
                    .get(AGENT_PLAN_SERVICE_ID)
                    .map_err(shared_error)?
                    .enter(None, false)
                    .await
                    .map_err(shared_error)?;
            }
            let context = handle.get(SESSION_CONTEXT_ID).map_err(shared_error)?;
            self.append_session_index_entry(&session_id, &options.work_dir, &context.workspace_id)
                .await
        }
        .await;
        if let Err(error) = initialized {
            let session_dir = handle
                .get(SESSION_CONTEXT_ID)
                .ok()
                .map(|context| PathBuf::from(&context.session_dir));
            self.inner
                .sessions
                .lock()
                .unwrap()
                .shift_remove(&session_id);
            let _ = self.drain_agents(&handle).await;
            let _ = handle.dispose();
            if let Some(session_dir) = session_dir {
                let _ = self.inner.host_fs.remove(&session_dir).await;
            }
            return Err(error);
        }
        self.announce_created(SessionCreatedEvent {
            session_id,
            handle: handle.clone(),
            source: SessionCreateSource::Startup,
        })
        .await?;
        Ok(handle)
    }

    fn get(&self, session_id: &str) -> Option<SessionScopeHandle> {
        if self.inner.resuming.lock().unwrap().contains_key(session_id) {
            return None;
        }
        self.inner.sessions.lock().unwrap().get(session_id).cloned()
    }

    fn list(&self) -> Vec<SessionScopeHandle> {
        let resuming = self.inner.resuming.lock().unwrap();
        self.inner
            .sessions
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| !resuming.contains_key(id.as_str()))
            .map(|(_, handle)| handle.clone())
            .collect()
    }

    async fn resume(&self, session_id: &str) -> ResumeResult {
        let in_flight = {
            let resuming = self.inner.resuming.lock().unwrap();
            resuming.get(session_id).map(|entry| entry.future.clone())
        };
        if let Some(future) = in_flight {
            return future.await;
        }
        if let Some(live) = self.inner.sessions.lock().unwrap().get(session_id).cloned() {
            return Ok(Some(live));
        }

        let generation = self
            .inner
            .next_resume_generation
            .fetch_add(1, Ordering::Relaxed);
        let lifecycle = self.clone();
        let id = session_id.to_owned();
        let future = async move {
            let result = lifecycle.do_resume(&id).await;
            if let Err(error) = &result {
                lifecycle
                    .inner
                    .telemetry
                    .set_context(&TelemetryContextPatch::from([(
                        "sessionId".into(),
                        Some(Value::String(id.clone())),
                    )]));
                let _ = lifecycle
                    .inner
                    .telemetry
                    .track_event(&SessionLoadFailedEvent {
                        reason: error_reason(error.as_ref()),
                    });
            }
            let mut resuming = lifecycle.inner.resuming.lock().unwrap();
            if resuming
                .get(&id)
                .is_some_and(|entry| entry.generation == generation)
            {
                resuming.remove(&id);
            }
            result
        }
        .boxed()
        .shared();
        self.inner.resuming.lock().unwrap().insert(
            session_id.into(),
            ResumeEntry {
                generation,
                future: future.clone(),
            },
        );
        future.await
    }

    async fn close(&self, session_id: &str) -> Result<(), SessionLifecycleError> {
        let handle = { self.inner.sessions.lock().unwrap().get(session_id).cloned() };
        let Some(handle) = handle else {
            return Ok(());
        };
        self.announce_will_close(SessionWillCloseEvent {
            session_id: session_id.into(),
            handle: handle.clone(),
            reason: SessionCloseReason::Exit,
        })
        .await?;
        self.inner.sessions.lock().unwrap().shift_remove(session_id);
        self.drain_agents(&handle).await?;
        handle.dispose().map_err(shared_error)?;
        self.inner.did_close.fire(&SessionClosedEvent {
            session_id: session_id.into(),
        });
        Ok(())
    }

    async fn archive(&self, session_id: &str) -> Result<(), SessionLifecycleError> {
        let handle = { self.inner.sessions.lock().unwrap().get(session_id).cloned() };
        let Some(handle) = handle else {
            return Ok(());
        };
        handle
            .get(SESSION_METADATA_ID)
            .map_err(shared_error)?
            .set_archived(true)
            .await
            .map_err(boxed_error)?;
        self.drain_agents(&handle).await?;
        self.inner.event.publish(GlobalDomainEvent {
            event_type: "event.session.archived".into(),
            payload: json!({"sessionId": session_id}),
        });
        self.announce_will_close(SessionWillCloseEvent {
            session_id: session_id.into(),
            handle: handle.clone(),
            reason: SessionCloseReason::Exit,
        })
        .await?;
        self.inner.sessions.lock().unwrap().shift_remove(session_id);
        handle.dispose().map_err(shared_error)?;
        self.inner.did_archive.fire(&SessionArchivedEvent {
            session_id: session_id.into(),
        });
        Ok(())
    }

    async fn restore(&self, session_id: &str) -> ResumeResult {
        let Some(handle) = self.resume(session_id).await? else {
            return Ok(None);
        };
        handle
            .get(SESSION_METADATA_ID)
            .map_err(shared_error)?
            .set_archived(false)
            .await
            .map_err(boxed_error)?;
        Ok(Some(handle))
    }

    async fn fork(
        &self,
        options: ForkSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError> {
        let source_id = options.source_session_id;
        let source_handle = { self.inner.sessions.lock().unwrap().get(&source_id).cloned() };
        let index_summary = self
            .inner
            .index
            .get(&source_id)
            .await
            .map_err(boxed_error)?;
        if source_handle.is_none() && index_summary.is_none() {
            return Err(Arc::new(Error2::new(
                SESSION_NOT_FOUND,
                format!("session {source_id} does not exist"),
            )));
        }
        let workspace_id = match &source_handle {
            Some(handle) => handle
                .get(SESSION_CONTEXT_ID)
                .map_err(shared_error)?
                .workspace_id
                .clone(),
            None => index_summary
                .as_ref()
                .expect("checked above")
                .workspace_id
                .clone(),
        };

        let mut target_id = None;
        let mut target = None;
        let mut target_session_dir = None;
        let result = async {
            let workspace = self
                .inner
                .workspace_registry
                .get(&workspace_id)
                .await
                .map_err(boxed_error)?
                .ok_or_else(|| {
                    Arc::new(Error2::new(
                        WORKSPACE_NOT_FOUND,
                        format!("workspace {workspace_id} does not exist"),
                    )) as SessionLifecycleError
                })?;
            let source_meta = match &source_handle {
                Some(handle) => Some(
                    handle
                        .get(SESSION_METADATA_ID)
                        .map_err(shared_error)?
                        .read()
                        .await
                        .map_err(boxed_error)?,
                ),
                None => self.read_meta_from_disk(&workspace_id, &source_id).await?,
            };
            let id = options.new_session_id.unwrap_or_else(create_session_id);
            target_id = Some(id.clone());
            let live_exists = self.inner.sessions.lock().unwrap().contains_key(&id);
            if live_exists
                || self
                    .inner
                    .index
                    .get(&id)
                    .await
                    .map_err(boxed_error)?
                    .is_some()
            {
                return Err(Arc::new(Error2::new(
                    SESSION_ALREADY_EXISTS,
                    format!("Session \"{id}\" already exists"),
                )) as SessionLifecycleError);
            }

            let target_dir = self.inner.bootstrap.session_dir(&workspace_id, &id);
            target_session_dir = Some(target_dir.clone());
            self.copy_session_files(
                &self.inner.bootstrap.session_dir(&workspace_id, &source_id),
                &target_dir,
            )
            .await?;
            let handle = self
                .materialize_session(MaterializeSessionOptions {
                    session_id: id.clone(),
                    work_dir: workspace.root.clone(),
                    additional_dirs: None,
                    mcp_servers: None,
                    workspace_id: None,
                })
                .await?
                .expect("materialize always returns a handle");
            target = Some(handle.clone());
            let target_context = handle.get(SESSION_CONTEXT_ID).map_err(shared_error)?;
            let target_metadata = handle.get(SESSION_METADATA_ID).map_err(shared_error)?;
            let source_agents = source_meta
                .as_ref()
                .and_then(|meta| meta.agents.clone())
                .unwrap_or_default();
            for agent_id in source_agents.keys() {
                self.copy_agent_wire(CopyAgentWireOptions {
                    source_handle: source_handle.as_ref(),
                    source_workspace_id: &workspace_id,
                    source_session_id: &source_id,
                    agent_id,
                    target_workspace_id: &target_context.workspace_id,
                    target_session_id: &target_context.session_id,
                })
                .await?;
            }
            let source_title = source_meta
                .as_ref()
                .and_then(|meta| meta.title.as_deref())
                .filter(|title| !title.is_empty())
                .unwrap_or(&source_id);
            let custom = fork_custom_metadata(
                source_meta.as_ref().and_then(|meta| meta.custom.as_ref()),
                options.metadata.as_ref(),
            );
            target_metadata
                .update(SessionMetaPatch {
                    title: Some(
                        options
                            .title
                            .clone()
                            .unwrap_or_else(|| format!("Fork: {source_title}")),
                    ),
                    is_custom_title: Some(
                        options.title.is_some()
                            || source_meta.as_ref().and_then(|meta| meta.is_custom_title)
                                == Some(true),
                    ),
                    forked_from: Some(source_id.clone()),
                    archived: Some(false),
                    last_prompt: source_meta
                        .as_ref()
                        .and_then(|meta| meta.last_prompt.clone()),
                    custom,
                    ..SessionMetaPatch::default()
                })
                .await
                .map_err(boxed_error)?;
            self.duplicate_cron_tasks(&workspace_id, &source_id, &id)
                .await?;
            let target_agents = handle
                .get(AGENT_LIFECYCLE_SERVICE_ID)
                .map_err(shared_error)?;
            for (agent_id, source_agent) in source_agents {
                target_agents
                    .create(CreateAgentOptions {
                        agent_id: Some(agent_id),
                        forked_from: source_agent.forked_from.clone(),
                        labels: labels_from_agent_meta(&source_agent),
                        ..CreateAgentOptions::default()
                    })
                    .await
                    .map_err(Arc::from)?;
            }
            self.append_session_index_entry(&id, &workspace.root, &target_context.workspace_id)
                .await?;
            self.inner.did_fork.fire(&SessionForkedEvent {
                source_session_id: source_id.clone(),
                session_id: id.clone(),
                handle: handle.clone(),
            });
            self.announce_created(SessionCreatedEvent {
                session_id: id,
                handle: handle.clone(),
                source: SessionCreateSource::Fork,
            })
            .await?;
            Ok(handle)
        }
        .await;
        if let Err(error) = result {
            if let Some(id) = target_id {
                self.inner.sessions.lock().unwrap().shift_remove(&id);
            }
            if let Some(handle) = target {
                let _ = handle.dispose();
            }
            if let Some(directory) = target_session_dir {
                let _ = self.inner.host_fs.remove(&directory).await;
            }
            return Err(error);
        }
        result
    }

    async fn create_child(
        &self,
        options: CreateChildSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError> {
        let title = match options.title {
            Some(title) => title,
            None => format!(
                "Child: {}",
                self.resolve_source_title(&options.source_session_id)
                    .await?
                    .unwrap_or_else(|| options.source_session_id.clone())
            ),
        };
        let mut metadata = options.metadata.unwrap_or_default();
        metadata.insert(
            PARENT_SESSION_ID_KEY.into(),
            Value::String(options.source_session_id.clone()),
        );
        metadata.insert(
            CHILD_SESSION_KIND_KEY.into(),
            Value::String(CHILD_SESSION_KIND.into()),
        );
        self.fork(ForkSessionOptions {
            source_session_id: options.source_session_id,
            new_session_id: options.new_session_id,
            title: Some(title),
            metadata: Some(metadata),
        })
        .await
    }
}

impl Disposable for SessionLifecycleService {
    fn dispose(&self) -> DisposeResult {
        self.inner.did_create.dispose()?;
        self.inner.did_close.dispose()?;
        self.inner.did_archive.dispose()?;
        self.inner.did_fork.dispose()
    }
}

struct MaterializeSessionOptions {
    session_id: String,
    work_dir: String,
    additional_dirs: Option<Vec<String>>,
    mcp_servers: Option<HashMap<String, crate::agent::mcp::McpServerConfig>>,
    workspace_id: Option<String>,
}

struct CopyAgentWireOptions<'a> {
    source_handle: Option<&'a SessionScopeHandle>,
    source_workspace_id: &'a str,
    source_session_id: &'a str,
    agent_id: &'a str,
    target_workspace_id: &'a str,
    target_session_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LegacySessionIndexEntry<'a> {
    session_id: &'a str,
    session_dir: String,
    work_dir: &'a str,
}

fn boxed_error(error: Box<dyn std::error::Error + Send + Sync>) -> SessionLifecycleError {
    Arc::from(error)
}

fn shared_error(error: impl std::error::Error + Send + Sync + 'static) -> SessionLifecycleError {
    Arc::new(error)
}

fn create_session_id() -> String {
    format!("session_{}", Uuid::new_v4())
}

fn forked_record() -> WireRecord {
    Map::from_iter([
        ("type".into(), Value::String("forked".into())),
        ("time".into(), Value::from(now_millis())),
    ])
}

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn fork_custom_metadata(
    source: Option<&BTreeMap<String, Value>>,
    input: Option<&Map<String, Value>>,
) -> Option<BTreeMap<String, Value>> {
    let mut merged = source.cloned().unwrap_or_default();
    merged.remove("goal");
    if let Some(input) = input {
        for (key, value) in input {
            if key == "goal" {
                merged.remove("goal");
            } else {
                merged.insert(key.clone(), value.clone());
            }
        }
    }
    (!merged.is_empty()).then_some(merged)
}

fn error_reason(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<Error2>() {
        return error.code.clone();
    }
    "Error".into()
}

pub fn register_session_lifecycle_service() {
    register_scoped_service(
        LifecycleScope::App,
        SESSION_LIFECYCLE_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let service = SessionLifecycleService::new(
                (*accessor.get(INSTANTIATION_SERVICE_ID)?).clone(),
                (*accessor.get(BOOTSTRAP_SERVICE_ID)?).clone(),
                (*accessor.get(CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_ENVIRONMENT_SERVICE_ID)?).clone(),
                (*accessor.get(SESSION_INDEX_SERVICE_ID)?).clone(),
                (*accessor.get(APPEND_LOG_STORE_SERVICE_ID)?).clone(),
                (*accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?).clone(),
                (*accessor.get(HOST_FILE_SYSTEM_SERVICE_ID)?).clone(),
                (*accessor.get(CRON_TASK_PERSISTENCE_SERVICE_ID)?).clone(),
                (*accessor.get(WORKSPACE_REGISTRY_SERVICE_ID)?).clone(),
                (*accessor.get(WORKSPACE_LOCAL_CONFIG_SERVICE_ID)?).clone(),
                (*accessor.get(EVENT_SERVICE_ID)?).clone(),
                (*accessor.get(TELEMETRY_SERVICE_ID)?).clone(),
            );
            let contract: Arc<dyn SessionLifecycleServiceContract> = Arc::new(service);
            Ok(SessionLifecycleServiceHandle(contract))
        })
        .disposable(),
        InstantiationType::Eager,
        "sessionLifecycle",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_session_ids_match_the_source_shape() {
        let id = create_session_id();
        assert!(id.starts_with("session_"));
        assert_eq!(id, id.to_lowercase());
        assert!(Uuid::parse_str(id.trim_start_matches("session_")).is_ok());
    }

    #[test]
    fn fork_custom_metadata_drops_goal_from_both_inputs() {
        let source = BTreeMap::from([
            ("goal".into(), json!({"old": true})),
            ("source".into(), json!(1)),
        ]);
        let input = Map::from_iter([
            ("goal".into(), json!({"new": true})),
            ("input".into(), json!(2)),
        ]);
        assert_eq!(
            fork_custom_metadata(Some(&source), Some(&input)),
            Some(BTreeMap::from([
                ("input".into(), json!(2)),
                ("source".into(), json!(1)),
            ]))
        );
    }

    #[test]
    fn wire_fork_boundary_and_error_reasons_match_source() {
        assert_eq!(forked_record()["type"], "forked");
        assert!(forked_record()["time"].is_i64());
        assert_eq!(
            error_reason(&Error2::new(SESSION_NOT_FOUND, "missing")),
            SESSION_NOT_FOUND
        );
        assert_eq!(error_reason(&std::io::Error::other("failed")), "Error");
    }

    #[test]
    fn registration_is_eager_app_scoped_and_uses_source_domain() {
        crate::_base::di::scope::clear_scoped_registry_for_tests();
        register_session_lifecycle_service();
        let entries = crate::_base::di::scope::get_scoped_service_descriptors(LifecycleScope::App);
        assert!(entries.iter().any(|entry| {
            entry.id.to_string() == SESSION_LIFECYCLE_SERVICE_ID.to_string()
                && !entry.descriptor.supports_delayed_instantiation
                && entry.domain == "sessionLifecycle"
        }));
        crate::_base::di::scope::clear_scoped_registry_for_tests();
    }
}
