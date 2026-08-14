//! App-scoped v1-compatible message-history adapter.
//!
//! Original:
//! `packages/agent-core-v2/src/app/messageLegacy/messageLegacyService.ts`.

use std::{error::Error, fmt, sync::Arc};

use async_trait::async_trait;
use futures_util::StreamExt;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, ScopeHandle, register_scoped_service},
        },
        errors::errors::Error2,
    },
    agent::{
        blob::AGENT_BLOB_SERVICE_ID,
        context_memory::{
            AGENT_CONTEXT_MEMORY_SERVICE_ID, ContextMessage, ContextTranscript,
            create_context_transcript_reducer, protocol_message::ProtocolMessage,
            to_protocol_message,
        },
        scope_context::AGENT_SCOPE_CONTEXT_ID,
    },
    app::{
        session_index::{SESSION_INDEX_SERVICE_ID, SessionIndexHandle, SessionSummary},
        session_lifecycle::{
            SESSION_LIFECYCLE_SERVICE_ID, SessionLifecycleError, SessionLifecycleServiceHandle,
        },
    },
    persistence::interface::append_log_store::{APPEND_LOG_STORE_SERVICE_ID, AppendLogStoreHandle},
    session::{
        agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, AGENT_NOT_FOUND, CreateAgentOptions, MAIN_AGENT_ID,
            ensure_main_agent, labels_from_agent_meta,
        },
        session_metadata::SESSION_METADATA_ID,
    },
    wire::{
        contract::WIRE_SERVICE_ID,
        record::{AGENT_WIRE_RECORD_KEY, WireRecord},
    },
};

use super::{
    MESSAGE_LEGACY_SERVICE_ID, MESSAGE_NOT_FOUND, MessageLegacyResult,
    MessageLegacyServiceContract, MessageLegacyServiceHandle, MessageListQuery, PageResponse,
    ensure_message_legacy_errors_registered,
};

const SESSION_NOT_FOUND: &str = "session.not_found";
const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 100;

#[derive(Debug)]
struct SharedLifecycleError(SessionLifecycleError);

impl fmt::Display for SharedLifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl Error for SharedLifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pub struct MessageLegacyService {
    lifecycle: SessionLifecycleServiceHandle,
    index: SessionIndexHandle,
    append_log: AppendLogStoreHandle,
}

impl MessageLegacyService {
    pub fn new(
        lifecycle: SessionLifecycleServiceHandle,
        index: SessionIndexHandle,
        append_log: AppendLogStoreHandle,
    ) -> Self {
        ensure_message_legacy_errors_registered();
        Self {
            lifecycle,
            index,
            append_log,
        }
    }

    async fn load_messages(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> MessageLegacyResult<Vec<ProtocolMessage>> {
        let Some(summary) = self.index.get(session_id).await? else {
            return Err(Box::new(Error2::new(
                SESSION_NOT_FOUND,
                format!("session {session_id} does not exist"),
            )));
        };

        let session = self.lifecycle.resume(session_id).await.map_err(|error| {
            Box::new(SharedLifecycleError(error)) as Box<dyn Error + Send + Sync>
        })?;
        let Some(session) = session else {
            return Ok(Vec::new());
        };
        let agent_id = agent_id.unwrap_or(MAIN_AGENT_ID);
        let agent = if agent_id == MAIN_AGENT_ID {
            ensure_main_agent(&session, None).await?
        } else {
            let metadata = session.get(SESSION_METADATA_ID)?;
            let persisted = metadata.read().await?;
            let meta = persisted
                .agents
                .as_ref()
                .and_then(|agents| agents.get(agent_id))
                .cloned()
                .ok_or_else(|| {
                    Box::new(Error2::new(
                        AGENT_NOT_FOUND,
                        format!("agent {agent_id} does not exist in session {session_id}"),
                    )) as Box<dyn Error + Send + Sync>
                })?;
            let lifecycle = session.get(AGENT_LIFECYCLE_SERVICE_ID)?;
            match lifecycle.get(agent_id) {
                Some(agent) => agent,
                None => {
                    lifecycle
                        .create(CreateAgentOptions {
                            agent_id: Some(agent_id.into()),
                            forked_from: meta.forked_from.clone(),
                            labels: labels_from_agent_meta(&meta),
                            ..CreateAgentOptions::default()
                        })
                        .await?
                }
            }
        };

        let transcript = self.read_transcript(&agent).await?;
        let context_messages = agent.get(AGENT_CONTEXT_MEMORY_SERVICE_ID)?.get();
        let merged = merge_live_tail(transcript, &context_messages);
        let entries = rehydrate(&agent, merged.messages).await?;
        project_messages(session_id, &summary, entries, &merged.times)
    }

    async fn read_transcript(&self, agent: &ScopeHandle) -> MessageLegacyResult<ContextTranscript> {
        agent.get(WIRE_SERVICE_ID)?.flush().await?;
        let scope = agent.get(AGENT_SCOPE_CONTEXT_ID)?.scope(None);
        let mut reducer = create_context_transcript_reducer();
        let mut records = self
            .append_log
            .read::<WireRecord>(&scope, AGENT_WIRE_RECORD_KEY);
        while let Some(record) = records.next().await {
            reducer.add(&record?);
        }
        Ok(reducer.result())
    }
}

#[async_trait]
impl MessageLegacyServiceContract for MessageLegacyService {
    async fn list_all(
        &self,
        session_id: &str,
        agent_id: Option<&str>,
    ) -> MessageLegacyResult<Vec<ProtocolMessage>> {
        self.load_messages(session_id, agent_id).await
    }

    async fn list(
        &self,
        session_id: &str,
        query: MessageListQuery,
    ) -> MessageLegacyResult<PageResponse<ProtocolMessage>> {
        let mut messages = self
            .load_messages(session_id, query.agent_id.as_deref())
            .await?;
        messages.reverse();

        let pivot_index = if let Some(before_id) = query.before_id.as_deref() {
            messages.iter().position(|message| message.id == before_id)
        } else if let Some(after_id) = query.after_id.as_deref() {
            messages.iter().position(|message| message.id == after_id)
        } else {
            None
        };
        let (start, end) = if query.before_id.is_some() {
            (pivot_index.map_or(0, |index| index + 1), messages.len())
        } else if query.after_id.is_some() {
            (0, pivot_index.unwrap_or(messages.len()))
        } else {
            (0, messages.len())
        };
        let slice = &messages[start..end];

        let page_size = query
            .page_size
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let has_more = slice.len() > page_size;
        let mut items = slice.iter().take(page_size).cloned().collect::<Vec<_>>();
        if let Some(role) = query.role {
            items.retain(|message| message.role == role);
        }

        Ok(PageResponse { items, has_more })
    }

    async fn get(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> MessageLegacyResult<ProtocolMessage> {
        self.load_messages(session_id, None)
            .await?
            .into_iter()
            .find(|message| message.id == message_id)
            .ok_or_else(|| {
                Box::new(Error2::new(
                    MESSAGE_NOT_FOUND,
                    format!("message {message_id} does not exist in session {session_id}"),
                )) as Box<dyn Error + Send + Sync>
            })
    }
}

struct MergedHistory {
    messages: Vec<ContextMessage>,
    times: Vec<Option<i64>>,
}

fn merge_live_tail(
    transcript: ContextTranscript,
    context_messages: &[ContextMessage],
) -> MergedHistory {
    if context_messages.len() as u64 <= transcript.folded_length {
        return MergedHistory {
            messages: transcript.entries,
            times: transcript.times,
        };
    }

    let tail_start = transcript.folded_length as usize;
    let tail = context_messages.iter().skip(tail_start);
    let mut messages = transcript.entries;
    let mut times = transcript.times;
    for message in tail {
        messages.push(message.clone());
        times.push(None);
    }
    MergedHistory { messages, times }
}

async fn rehydrate(
    agent: &ScopeHandle,
    messages: Vec<ContextMessage>,
) -> MessageLegacyResult<Vec<ContextMessage>> {
    let blobs = agent.get(AGENT_BLOB_SERVICE_ID)?;
    let mut output = Vec::with_capacity(messages.len());
    for mut message in messages {
        let content = std::mem::take(&mut message.message.content);
        message.message.content = blobs.load_parts(content).await;
        output.push(message);
    }
    Ok(output)
}

fn project_messages(
    session_id: &str,
    summary: &SessionSummary,
    messages: Vec<ContextMessage>,
    times: &[Option<i64>],
) -> MessageLegacyResult<Vec<ProtocolMessage>> {
    let mut previous_ms = f64::NEG_INFINITY;
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let base_ms = times
                .get(index)
                .copied()
                .flatten()
                .map(|value| value as f64)
                .unwrap_or(summary.created_at as f64 + index as f64);
            let created_at_ms = (previous_ms + 1.0).max(base_ms);
            previous_ms = created_at_ms;
            let created_at_ms = time_clip(created_at_ms)?;
            Ok(to_protocol_message(
                session_id,
                index as u64,
                message,
                time_clip(summary.created_at as f64)?,
                Some(created_at_ms),
            )?)
        })
        .collect()
}

fn time_clip(value: f64) -> MessageLegacyResult<i64> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(Box::new(
            crate::agent::context_memory::MessageProjectionError::InvalidTimestamp,
        ));
    }
    Ok(value.trunc() as i64)
}

pub fn register_message_legacy_service() {
    register_scoped_service(
        LifecycleScope::App,
        MESSAGE_LEGACY_SERVICE_ID,
        SyncDescriptor::new(|accessor| {
            let lifecycle = accessor.get(SESSION_LIFECYCLE_SERVICE_ID)?;
            let index = accessor.get(SESSION_INDEX_SERVICE_ID)?;
            let append_log = accessor.get(APPEND_LOG_STORE_SERVICE_ID)?;
            let service: Arc<dyn MessageLegacyServiceContract> =
                Arc::new(MessageLegacyService::new(
                    (*lifecycle).clone(),
                    (*index).clone(),
                    (*append_log).clone(),
                ));
            Ok(MessageLegacyServiceHandle(service))
        }),
        InstantiationType::Eager,
        "messageLegacy",
    );
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use futures_util::{FutureExt, future::BoxFuture, stream};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        _base::{
            di::{
                lifecycle::{Disposable, DisposeResult, disposable_none},
                scope::{LifecycleScope, Scope, ScopeOptions},
                service_collection::ServiceCollection,
            },
            event::Event,
            lifecycle::lifecycle_machine::BoxError,
        },
        agent::{
            blob::{AgentBlobServiceContract, AgentBlobServiceHandle},
            context_memory::{
                AgentContextMemoryServiceContract, AgentContextMemoryServiceHandle,
                ContextCompactionInput, ContextCompactionResult, ContextMemoryServiceError,
                LoopRecordedEvent, MessageContent, MessageRole, UndoCut,
            },
            permission_policy::PermissionMode,
            scope_context::{AgentScopeContextInput, make_agent_scope_context},
        },
        app::{
            session_index::{SessionIndexContract, SessionIndexResult, SessionListQuery},
            session_lifecycle::{
                CreateChildSessionOptions, CreateSessionOptions, ForkSessionOptions,
                SessionArchivedEvent, SessionClosedEvent, SessionCreatedEvent, SessionForkedEvent,
                SessionLifecycleHooks, SessionLifecycleServiceContract,
                SessionLifecycleServiceHandle, SessionScopeHandle,
            },
        },
        kosong::contract::message::{ContentPart, Message, Role},
        persistence::interface::{
            append_log_store::{
                AppendLogError, AppendLogOptions, AppendLogStoreService, AppendLogValueStream,
            },
            query_store::Page,
            storage::StorageError,
        },
        session::agent_lifecycle::{
            AGENT_LIFECYCLE_SERVICE_ID, AgentLifecycleServiceContract, AgentLifecycleServiceHandle,
            AgentListFilter, AgentScopeHandle, CreateAgentOptions, ForkAgentOptions, MAIN_AGENT_ID,
        },
        session::session_metadata::{
            AgentMeta, AgentMetaType, SessionMeta, SessionMetaPatch, SessionMetadataChangedEvent,
            SessionMetadataContract, SessionMetadataHandle,
        },
        wire::{
            contract::{WIRE_SERVICE_ID, WireServiceHandle},
            wire_service::{DomainEventPublisher, WireBlobService, WireService},
        },
    };

    #[derive(Default)]
    struct MemoryLog {
        records: Mutex<Vec<Value>>,
    }

    impl MemoryLog {
        fn new(records: Vec<Value>) -> Self {
            Self {
                records: Mutex::new(records),
            }
        }
    }

    #[async_trait]
    impl AppendLogStoreService for MemoryLog {
        fn append_value(&self, _: &str, _: &str, record: Value, _: AppendLogOptions) {
            self.records.lock().unwrap().push(record);
        }

        fn read_values(&self, _: &str, _: &str) -> AppendLogValueStream {
            Box::pin(stream::iter(
                self.records.lock().unwrap().clone().into_iter().map(Ok),
            ))
        }

        async fn rewrite_values(
            &self,
            _: &str,
            _: &str,
            records: Vec<Value>,
        ) -> Result<(), AppendLogError> {
            *self.records.lock().unwrap() = records;
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

    struct IdentityWireBlobs;

    #[async_trait]
    impl WireBlobService for IdentityWireBlobs {
        async fn offload_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    struct NoopEvents;

    impl DomainEventPublisher for NoopEvents {
        fn publish(&self, _: Value) {}
    }

    struct Context {
        messages: Vec<ContextMessage>,
    }

    impl AgentContextMemoryServiceContract for Context {
        fn get(&self) -> crate::agent::context_memory::ContextMemorySnapshot {
            self.messages.clone().into()
        }

        fn append(&self, _: Vec<ContextMessage>) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn append_loop_event(&self, _: LoopRecordedEvent) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn clear(&self) -> Result<(), ContextMemoryServiceError> {
            Ok(())
        }

        fn undo(&self, _: u32) -> Result<UndoCut, ContextMemoryServiceError> {
            unreachable!("message history tests do not mutate context")
        }

        fn apply_compaction(
            &self,
            _: ContextCompactionInput,
        ) -> Result<ContextCompactionResult, ContextMemoryServiceError> {
            unreachable!("message history tests do not compact context")
        }
    }

    struct Blobs {
        hydrate: bool,
    }

    #[async_trait]
    impl AgentBlobServiceContract for Blobs {
        async fn offload_parts(
            &self,
            parts: Vec<ContentPart>,
        ) -> Result<Vec<ContentPart>, StorageError> {
            Ok(parts)
        }

        async fn load_parts(&self, mut parts: Vec<ContentPart>) -> Vec<ContentPart> {
            if self.hydrate {
                for part in &mut parts {
                    if let ContentPart::ImageUrl { image_url } = part
                        && image_url.url.starts_with("blobref:")
                    {
                        image_url.url = "data:image/png;base64,AAAA".into();
                    }
                }
            }
            parts
        }

        fn is_blob_ref(&self, url: &str) -> bool {
            url.starts_with("blobref:")
        }

        async fn offload_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }

        async fn load_wire_parts(&self, parts: Vec<Value>) -> Result<Vec<Value>, String> {
            Ok(parts)
        }
    }

    #[derive(Default)]
    struct AgentLifecycle {
        agent: Mutex<Option<AgentScopeHandle>>,
        created: Mutex<Vec<String>>,
    }

    impl Disposable for AgentLifecycle {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    impl AgentLifecycleServiceContract for AgentLifecycle {
        fn on_did_create(&self) -> Event<AgentScopeHandle> {
            Event::none()
        }

        fn on_did_dispose(&self) -> Event<String> {
            Event::none()
        }

        fn create(
            &self,
            options: CreateAgentOptions,
        ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
            if let Some(agent_id) = options.agent_id {
                self.created.lock().unwrap().push(agent_id);
            }
            futures_util::future::ready(Ok(self
                .agent
                .lock()
                .unwrap()
                .clone()
                .expect("test agent is installed")))
            .boxed()
        }

        fn fork(
            &self,
            _: String,
            _: ForkAgentOptions,
        ) -> BoxFuture<'static, Result<AgentScopeHandle, BoxError>> {
            unreachable!("message history tests do not fork agents")
        }

        fn get(&self, id: &str) -> Option<AgentScopeHandle> {
            (id == MAIN_AGENT_ID)
                .then(|| self.agent.lock().unwrap().clone())
                .flatten()
        }

        fn list(&self, _: Option<&AgentListFilter>) -> Vec<AgentScopeHandle> {
            self.agent.lock().unwrap().iter().cloned().collect()
        }

        fn broadcast_permission_mode(&self, _: PermissionMode) -> Result<(), BoxError> {
            Ok(())
        }

        fn remove(&self, _: String) -> BoxFuture<'static, Result<(), BoxError>> {
            futures_util::future::ready(Ok(())).boxed()
        }
    }

    struct Metadata {
        data: SessionMeta,
    }

    #[async_trait]
    impl SessionMetadataContract for Metadata {
        async fn ready(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
            Event::none()
        }

        async fn read(&self) -> Result<SessionMeta, Box<dyn std::error::Error + Send + Sync>> {
            Ok(self.data.clone())
        }

        async fn update(
            &self,
            _: SessionMetaPatch,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn set_title(
            &self,
            _: String,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn set_archived(
            &self,
            _: bool,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }

        async fn register_agent(
            &self,
            _: String,
            _: AgentMeta,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }
    }

    struct Lifecycle {
        session_id: String,
        session: SessionScopeHandle,
        hooks: SessionLifecycleHooks,
    }

    impl Disposable for Lifecycle {
        fn dispose(&self) -> DisposeResult {
            Ok(())
        }
    }

    #[async_trait]
    impl SessionLifecycleServiceContract for Lifecycle {
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
            Ok(self.session.clone())
        }

        fn get(&self, session_id: &str) -> Option<SessionScopeHandle> {
            (session_id == self.session_id).then(|| self.session.clone())
        }

        fn list(&self) -> Vec<SessionScopeHandle> {
            vec![self.session.clone()]
        }

        async fn resume(
            &self,
            session_id: &str,
        ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError> {
            Ok(self.get(session_id))
        }

        async fn close(&self, _: &str) -> Result<(), SessionLifecycleError> {
            Ok(())
        }

        async fn archive(&self, _: &str) -> Result<(), SessionLifecycleError> {
            Ok(())
        }

        async fn restore(
            &self,
            session_id: &str,
        ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError> {
            Ok(self.get(session_id))
        }

        async fn delete_archived(&self, _: &str) -> Result<bool, SessionLifecycleError> {
            Ok(false)
        }

        async fn fork(
            &self,
            _: ForkSessionOptions,
        ) -> Result<SessionScopeHandle, SessionLifecycleError> {
            unreachable!("message history tests do not fork sessions")
        }

        async fn create_child(
            &self,
            _: CreateChildSessionOptions,
        ) -> Result<SessionScopeHandle, SessionLifecycleError> {
            unreachable!("message history tests do not create child sessions")
        }
    }

    struct Index {
        summary: SessionSummary,
    }

    #[async_trait]
    impl SessionIndexContract for Index {
        async fn list(&self, _: SessionListQuery) -> SessionIndexResult<Page<SessionSummary>> {
            unreachable!("message history tests only get sessions by id")
        }

        async fn get(&self, id: &str) -> SessionIndexResult<Option<SessionSummary>> {
            Ok((id == self.summary.id).then(|| self.summary.clone()))
        }

        async fn remove(&self, _: &str) -> SessionIndexResult<()> {
            Ok(())
        }

        async fn count_active(&self, _: &[String]) -> SessionIndexResult<usize> {
            unreachable!("message history tests do not count sessions")
        }
    }

    struct Fixture {
        service: MessageLegacyService,
        agent_lifecycle: Arc<AgentLifecycle>,
        _app: Scope,
    }

    fn text_message(role: Role, text: &str) -> ContextMessage {
        ContextMessage {
            message: Message::new(
                role,
                vec![ContentPart::Text { text: text.into() }],
                Vec::new(),
            ),
            id: None,
            provider_message_id: None,
            origin: None,
            is_error: None,
            note: None,
            attachments: Vec::new(),
        }
    }

    fn summary() -> SessionSummary {
        SessionSummary {
            id: "s1".into(),
            workspace_id: "wd".into(),
            cwd: None,
            title: None,
            last_prompt: None,
            created_at: 1_000,
            updated_at: 1_000,
            archived: false,
            custom: None,
        }
    }

    fn fixture(
        records: Vec<Value>,
        context_messages: Vec<ContextMessage>,
        hydrate: bool,
    ) -> Fixture {
        let log = Arc::new(MemoryLog::new(records));
        let append_log_service: Arc<dyn AppendLogStoreService> = log.clone();
        let append_log = AppendLogStoreHandle(append_log_service);
        let wire = Arc::new(WireService::new(
            "sessions/wd/s1/agents/main",
            append_log.clone(),
            Arc::new(IdentityWireBlobs),
            Arc::new(NoopEvents),
        ));

        let agent_lifecycle = Arc::new(AgentLifecycle::default());
        let mut session_extra = ServiceCollection::new();
        let lifecycle_handle: Arc<dyn AgentLifecycleServiceContract> = agent_lifecycle.clone();
        session_extra.set_instance(
            AGENT_LIFECYCLE_SERVICE_ID,
            Arc::new(AgentLifecycleServiceHandle(lifecycle_handle)),
        );
        let metadata: Arc<dyn SessionMetadataContract> = Arc::new(Metadata {
            data: SessionMeta {
                id: "s1".into(),
                version: Some(2),
                title: None,
                is_custom_title: None,
                last_prompt: None,
                created_at: 1_000,
                updated_at: 1_000,
                archived: false,
                cwd: None,
                forked_from: None,
                agents: Some(BTreeMap::from([
                    (
                        MAIN_AGENT_ID.into(),
                        AgentMeta {
                            r#type: Some(AgentMetaType::Main),
                            ..AgentMeta::default()
                        },
                    ),
                    (
                        "agent-1".into(),
                        AgentMeta {
                            r#type: Some(AgentMetaType::Sub),
                            parent_agent_id: Some(MAIN_AGENT_ID.into()),
                            ..AgentMeta::default()
                        },
                    ),
                ])),
                custom: None,
            },
        });
        session_extra.set_instance(
            SESSION_METADATA_ID,
            Arc::new(SessionMetadataHandle(metadata)),
        );
        let app = Scope::create_app(ScopeOptions::default());
        let session = app
            .create_child(
                LifecycleScope::Session,
                "s1",
                ScopeOptions {
                    id: None,
                    extra: session_extra,
                },
            )
            .unwrap();

        let mut agent_extra = ServiceCollection::new();
        agent_extra.set_instance(
            WIRE_SERVICE_ID,
            Arc::new(WireServiceHandle(Arc::clone(&wire))),
        );
        let context: Arc<dyn AgentContextMemoryServiceContract> = Arc::new(Context {
            messages: context_messages,
        });
        agent_extra.set_instance(
            AGENT_CONTEXT_MEMORY_SERVICE_ID,
            Arc::new(AgentContextMemoryServiceHandle(context)),
        );
        let blobs: Arc<dyn AgentBlobServiceContract> = Arc::new(Blobs { hydrate });
        agent_extra.set_instance(
            AGENT_BLOB_SERVICE_ID,
            Arc::new(AgentBlobServiceHandle(blobs)),
        );
        agent_extra.set_instance(
            AGENT_SCOPE_CONTEXT_ID,
            Arc::new(make_agent_scope_context(AgentScopeContextInput {
                agent_id: MAIN_AGENT_ID.into(),
                agent_scope: "sessions/wd/s1/agents/main".into(),
            })),
        );
        let agent = session
            .create_child(
                LifecycleScope::Agent,
                MAIN_AGENT_ID,
                ScopeOptions {
                    id: None,
                    extra: agent_extra,
                },
            )
            .unwrap();
        *agent_lifecycle.agent.lock().unwrap() = Some(agent.to_handle());

        let lifecycle: Arc<dyn SessionLifecycleServiceContract> = Arc::new(Lifecycle {
            session_id: "s1".into(),
            session: session.to_handle(),
            hooks: SessionLifecycleHooks::default(),
        });
        let index: Arc<dyn SessionIndexContract> = Arc::new(Index { summary: summary() });
        Fixture {
            service: MessageLegacyService::new(
                SessionLifecycleServiceHandle(lifecycle),
                SessionIndexHandle(index),
                append_log,
            ),
            agent_lifecycle,
            _app: app,
        }
    }

    #[tokio::test]
    async fn reduces_streamed_history_and_merges_the_unflushed_live_tail() {
        let user = text_message(Role::User, "hi");
        let assistant = text_message(Role::Assistant, "hello");
        let fixture = fixture(
            vec![json!({"type": "context.append_message", "message": user})],
            vec![user, assistant],
            false,
        );

        let page = fixture
            .service
            .list("s1", MessageListQuery::default())
            .await
            .unwrap();

        assert_eq!(
            page.items
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            [MessageRole::Assistant, MessageRole::User]
        );
        assert_eq!(
            page.items[1].content,
            [MessageContent::Text { text: "hi".into() }]
        );
        assert!(!page.has_more);
    }

    #[tokio::test]
    async fn reports_coded_missing_session_and_message_errors() {
        let fixture = fixture(Vec::new(), Vec::new(), false);
        let error = fixture
            .service
            .list("missing", MessageListQuery::default())
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error2>().unwrap().code,
            SESSION_NOT_FOUND
        );

        let error = fixture.service.get("s1", "missing").await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error2>().unwrap().code,
            MESSAGE_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn reads_registered_subagent_history_and_rejects_unknown_agents() {
        let fixture = fixture(
            vec![json!({
                "type": "context.append_message",
                "message": text_message(Role::Assistant, "child response")
            })],
            Vec::new(),
            false,
        );
        let page = fixture
            .service
            .list(
                "s1",
                MessageListQuery {
                    agent_id: Some("agent-1".into()),
                    ..MessageListQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(
            fixture.agent_lifecycle.created.lock().unwrap().as_slice(),
            ["agent-1"]
        );

        let error = fixture
            .service
            .list(
                "s1",
                MessageListQuery {
                    agent_id: Some("agent-outside".into()),
                    ..MessageListQuery::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Error2>().unwrap().code,
            AGENT_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn resolves_derived_ids_and_rehydrates_blob_media() {
        let fixture = fixture(
            vec![
                json!({
                    "type": "context.append_message",
                    "message": text_message(Role::User, "hi")
                }),
                json!({
                    "type": "context.append_message",
                    "message": {
                        "role": "assistant",
                        "content": [{
                            "type": "image_url",
                            "imageUrl": {"url": "blobref:image/png;deadbeef"}
                        }],
                        "toolCalls": []
                    }
                }),
            ],
            Vec::new(),
            true,
        );

        let message = fixture.service.get("s1", "msg_s1_000001").await.unwrap();

        assert_eq!(message.role, MessageRole::Assistant);
        assert_eq!(
            message.content,
            [MessageContent::Image {
                source: crate::agent::context_memory::protocol_message::ImageSource::Url {
                    url: "data:image/png;base64,AAAA".into()
                }
            }]
        );
    }

    #[tokio::test]
    async fn preserves_media_and_text_tool_result_shapes() {
        let records = vec![
            json!({"type": "context.append_loop_event", "event": {"type": "step.begin", "uuid": "st1"}}),
            json!({"type": "context.append_loop_event", "event": {
                "type": "tool.call", "stepUuid": "st1", "toolCallId": "media", "name": "ReadMediaFile", "args": {}
            }}),
            json!({"type": "context.append_loop_event", "event": {
                "type": "tool.result", "toolCallId": "media", "result": {
                    "output": [{"type": "image_url", "imageUrl": {"url": "data:image/png;base64,AAAA"}}],
                    "isError": false
                }
            }}),
            json!({"type": "context.append_loop_event", "event": {"type": "step.end", "uuid": "st1"}}),
            json!({"type": "context.append_loop_event", "event": {"type": "step.begin", "uuid": "st2"}}),
            json!({"type": "context.append_loop_event", "event": {
                "type": "tool.call", "stepUuid": "st2", "toolCallId": "text", "name": "Bash"
            }}),
            json!({"type": "context.append_loop_event", "event": {
                "type": "tool.result", "toolCallId": "text", "result": {
                    "output": "command output", "isError": false
                }
            }}),
            json!({"type": "context.append_loop_event", "event": {"type": "step.end", "uuid": "st2"}}),
        ];
        let fixture = fixture(records, Vec::new(), false);

        let page = fixture
            .service
            .list("s1", MessageListQuery::default())
            .await
            .unwrap();
        let tool_results = page
            .items
            .iter()
            .filter_map(|message| match message.content.first() {
                Some(MessageContent::ToolResult { output, .. }) => Some(output),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(tool_results.len(), 2);
        assert_eq!(tool_results[0], &Value::String("command output".into()));
        assert!(tool_results[1].is_array());
    }

    #[tokio::test]
    async fn uses_record_times_nudged_to_remain_strictly_increasing() {
        let fixture = fixture(
            vec![
                json!({"type": "context.append_message", "message": text_message(Role::User, "u1"), "time": 5000}),
                json!({"type": "context.append_message", "message": text_message(Role::Assistant, "a1"), "time": 5000}),
                json!({"type": "context.append_message", "message": text_message(Role::User, "u2")}),
            ],
            Vec::new(),
            false,
        );

        let page = fixture
            .service
            .list("s1", MessageListQuery::default())
            .await
            .unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|message| message.created_at.as_str())
                .collect::<Vec<_>>(),
            [
                "1970-01-01T00:00:05.002Z",
                "1970-01-01T00:00:05.001Z",
                "1970-01-01T00:00:05.000Z"
            ]
        );
    }

    #[tokio::test]
    async fn paginates_newest_first_before_filtering_by_role() {
        let fixture = fixture(
            ["u1", "a1", "u2", "a2"]
                .into_iter()
                .enumerate()
                .map(|(index, text)| {
                    let role = if index % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    };
                    json!({
                        "type": "context.append_message",
                        "message": text_message(role, text)
                    })
                })
                .collect(),
            Vec::new(),
            false,
        );

        let first = fixture
            .service
            .list(
                "s1",
                MessageListQuery {
                    page_size: Some(2),
                    ..MessageListQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["msg_s1_000003", "msg_s1_000002"]
        );
        assert!(first.has_more);

        let older = fixture
            .service
            .list(
                "s1",
                MessageListQuery {
                    before_id: Some("msg_s1_000002".into()),
                    page_size: Some(2),
                    role: Some(MessageRole::Assistant),
                    ..MessageListQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            older
                .items
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["msg_s1_000001"]
        );
        assert!(!older.has_more);
    }

    #[tokio::test]
    async fn lists_all_messages_in_chronological_order_without_the_page_size_limit() {
        let fixture = fixture(
            (0..=MAX_PAGE_SIZE)
                .map(|index| {
                    json!({
                        "type": "context.append_message",
                        "message": text_message(Role::User, &format!("u{index}"))
                    })
                })
                .collect(),
            Vec::new(),
            false,
        );

        let messages = fixture.service.list_all("s1", None).await.unwrap();

        assert_eq!(messages.len(), MAX_PAGE_SIZE + 1);
        assert_eq!(messages.first().unwrap().id, "msg_s1_000000");
        assert_eq!(messages.last().unwrap().id, "msg_s1_000100");
    }
}
