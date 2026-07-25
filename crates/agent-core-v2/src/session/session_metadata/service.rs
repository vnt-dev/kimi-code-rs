use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{Emitter, Event},
    },
    persistence::interface::atomic_document_store::{
        ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle,
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

use super::{
    AgentMeta, SESSION_META_VERSION, SESSION_METADATA_ID, SessionMeta, SessionMetaPatch,
    SessionMetadataChangedEvent, SessionMetadataContract, SessionMetadataError,
    SessionMetadataHandle,
};

const META_KEY: &str = "state.json";

pub struct SessionMetadataService {
    context: SessionContext,
    store: AtomicDocumentStoreHandle,
    data: Mutex<Option<SessionMeta>>,
    load_lock: AsyncMutex<()>,
    update_lock: AsyncMutex<()>,
    changed: Arc<Emitter<SessionMetadataChangedEvent>>,
}

impl SessionMetadataService {
    pub fn new(context: &SessionContext, store: AtomicDocumentStoreHandle) -> Self {
        Self {
            context: context.clone(),
            store,
            data: Mutex::new(None),
            load_lock: AsyncMutex::new(()),
            update_lock: AsyncMutex::new(()),
            changed: Arc::new(Emitter::new()),
        }
    }
    pub fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
        self.changed.event()
    }
    pub async fn ready(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_loaded().await
    }
    pub async fn read(&self) -> Result<SessionMeta, Box<dyn std::error::Error + Send + Sync>> {
        self.ensure_loaded().await?;
        Ok(self.data.lock().unwrap().clone().expect("loaded metadata"))
    }
    pub async fn update(
        &self,
        patch: SessionMetaPatch,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lock = self.update_lock.lock().await;
        self.ensure_loaded().await?;
        let mut data = self.data.lock().unwrap().clone().expect("loaded metadata");
        let changed = patch_keys(&patch);
        apply_patch(&mut data, patch);
        data.updated_at = now_ms();
        self.store
            .set(&self.context.meta_scope, META_KEY, &data)
            .await?;
        *self.data.lock().unwrap() = Some(data);
        self.changed.fire(&SessionMetadataChangedEvent { changed });
        Ok(())
    }
    pub async fn set_title(
        &self,
        title: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update(SessionMetaPatch {
            title: Some(title),
            is_custom_title: Some(true),
            ..Default::default()
        })
        .await
    }
    pub async fn set_archived(
        &self,
        archived: bool,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.update(SessionMetaPatch {
            archived: Some(archived),
            ..Default::default()
        })
        .await
    }
    pub async fn register_agent(
        &self,
        agent_id: String,
        meta: AgentMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _lock = self.update_lock.lock().await;
        self.ensure_loaded().await?;
        let data = self.data.lock().unwrap().clone().expect("loaded metadata");
        if data
            .agents
            .as_ref()
            .and_then(|agents| agents.get(&agent_id))
            == Some(&meta)
        {
            return Ok(());
        }
        let mut agents = data.agents.clone().unwrap_or_default();
        agents.insert(agent_id, meta);
        drop(data);
        let mut current = self.data.lock().unwrap().clone().expect("loaded metadata");
        current.agents = Some(agents);
        current.updated_at = now_ms();
        self.store
            .set(&self.context.meta_scope, META_KEY, &current)
            .await?;
        *self.data.lock().unwrap() = Some(current);
        self.changed.fire(&SessionMetadataChangedEvent {
            changed: vec!["agents".into()],
        });
        Ok(())
    }
    async fn ensure_loaded(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if self.data.lock().unwrap().is_some() {
            return Ok(());
        }
        let _lock = self.load_lock.lock().await;
        if self.data.lock().unwrap().is_some() {
            return Ok(());
        }
        let loaded = self
            .store
            .get::<serde_json::Value>(&self.context.meta_scope, META_KEY)
            .await?;
        let created = loaded.is_none();
        let mut data = match loaded {
            Some(data) => {
                normalize_session_meta(session_meta_from_value(data)?, &self.context.session_id)
            }
            None => SessionMeta {
                id: self.context.session_id.clone(),
                version: Some(SESSION_META_VERSION),
                title: None,
                is_custom_title: None,
                last_prompt: None,
                created_at: now_ms(),
                updated_at: now_ms(),
                archived: false,
                cwd: Some(self.context.cwd.clone()),
                forked_from: None,
                agents: Some(BTreeMap::new()),
                custom: Some(BTreeMap::new()),
            },
        };
        let heal = data.agents.is_none() || data.custom.is_none();
        if data.agents.is_none() {
            data.agents = Some(BTreeMap::new());
        }
        if data.custom.is_none() {
            data.custom = Some(BTreeMap::new());
        }
        if created || heal {
            self.store
                .set(&self.context.meta_scope, META_KEY, &data)
                .await?;
        }
        *self.data.lock().unwrap() = Some(data);
        Ok(())
    }
}

#[async_trait]
impl SessionMetadataContract for SessionMetadataService {
    async fn ready(&self) -> Result<(), SessionMetadataError> {
        Self::ready(self).await
    }

    fn on_did_change_metadata(&self) -> Event<SessionMetadataChangedEvent> {
        Self::on_did_change_metadata(self)
    }

    async fn read(&self) -> Result<SessionMeta, SessionMetadataError> {
        Self::read(self).await
    }

    async fn update(&self, patch: SessionMetaPatch) -> Result<(), SessionMetadataError> {
        Self::update(self, patch).await
    }

    async fn set_title(&self, title: String) -> Result<(), SessionMetadataError> {
        Self::set_title(self, title).await
    }

    async fn set_archived(&self, archived: bool) -> Result<(), SessionMetadataError> {
        Self::set_archived(self, archived).await
    }

    async fn register_agent(
        &self,
        agent_id: String,
        meta: AgentMeta,
    ) -> Result<(), SessionMetadataError> {
        Self::register_agent(self, agent_id, meta).await
    }
}

/// Register the eager Session-scope metadata service.
///
/// Original: `registerScopedService(LifecycleScope.Session, ISessionMetadata, ...)`.
pub fn register_session_metadata() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_METADATA_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let store = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let service: Arc<dyn SessionMetadataContract> =
                Arc::new(SessionMetadataService::new(&context, (*store).clone()));
            Ok(SessionMetadataHandle(service))
        }),
        InstantiationType::Eager,
        "sessionMetadata",
    );
}

pub fn normalize_session_meta(mut raw: SessionMeta, session_id: &str) -> SessionMeta {
    if raw.version == Some(SESSION_META_VERSION) {
        return raw;
    }
    raw.id = session_id.into();
    raw.version = Some(SESSION_META_VERSION);
    raw
}
fn session_meta_from_value(mut value: serde_json::Value) -> Result<SessionMeta, serde_json::Error> {
    if let Some(object) = value.as_object_mut() {
        for key in ["createdAt", "updatedAt"] {
            if let Some(serde_json::Value::String(text)) = object.get(key) {
                let parsed = chrono::DateTime::parse_from_rfc3339(text)
                    .map(|date| date.timestamp_millis())
                    .unwrap_or_default();
                object.insert(key.into(), serde_json::Value::from(parsed));
            }
        }
    }
    serde_json::from_value(value)
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}
fn apply_patch(data: &mut SessionMeta, patch: SessionMetaPatch) {
    if let Some(value) = patch.version {
        data.version = Some(value)
    }
    if let Some(value) = patch.title {
        data.title = Some(value)
    }
    if let Some(value) = patch.is_custom_title {
        data.is_custom_title = Some(value)
    }
    if let Some(value) = patch.last_prompt {
        data.last_prompt = Some(value)
    }
    if let Some(value) = patch.archived {
        data.archived = value
    }
    if let Some(value) = patch.cwd {
        data.cwd = Some(value)
    }
    if let Some(value) = patch.forked_from {
        data.forked_from = Some(value)
    }
    if let Some(value) = patch.agents {
        data.agents = Some(value)
    }
    if let Some(value) = patch.custom {
        data.custom = Some(value)
    }
}
fn patch_keys(patch: &SessionMetaPatch) -> Vec<String> {
    let mut keys = Vec::new();
    if patch.version.is_some() {
        keys.push("version".into())
    }
    if patch.title.is_some() {
        keys.push("title".into())
    }
    if patch.is_custom_title.is_some() {
        keys.push("isCustomTitle".into())
    }
    if patch.last_prompt.is_some() {
        keys.push("lastPrompt".into())
    }
    if patch.archived.is_some() {
        keys.push("archived".into())
    }
    if patch.cwd.is_some() {
        keys.push("cwd".into())
    }
    if patch.forked_from.is_some() {
        keys.push("forkedFrom".into())
    }
    if patch.agents.is_some() {
        keys.push("agents".into())
    }
    if patch.custom.is_some() {
        keys.push("custom".into())
    }
    keys
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use async_trait::async_trait;
    use serde_json::Value;

    use super::*;
    use crate::{
        _base::{
            di::lifecycle::{DisposableHandle, disposable_none},
            event::Event,
        },
        persistence::interface::{
            atomic_document_store::AtomicDocumentStoreService, storage::StorageError,
        },
        session::session_context::{SessionContextInput, make_session_context},
    };

    #[derive(Default)]
    struct Store(Mutex<HashMap<(String, String), Value>>);
    #[async_trait]
    impl AtomicDocumentStoreService for Store {
        async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&(scope.into(), key.into()))
                .cloned())
        }
        async fn set_value(
            &self,
            scope: &str,
            key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            self.0
                .lock()
                .unwrap()
                .insert((scope.into(), key.into()), value);
            Ok(())
        }
        async fn delete(&self, _: &str, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
        async fn list(&self, _: &str, _: Option<&str>) -> Result<Vec<String>, StorageError> {
            Ok(vec![])
        }
        fn watch(&self, _: &str, _: &str) -> Event<()> {
            Event::none()
        }
        fn acquire(&self, _: &str, _: &str) -> DisposableHandle {
            disposable_none()
        }
    }
    fn service(store: Arc<Store>) -> SessionMetadataService {
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/s".into(),
            session_scope: "sessions/w/s".into(),
            cwd: "/repo".into(),
            meta_scope: None,
        });
        SessionMetadataService::new(&context, AtomicDocumentStoreHandle(store))
    }
    #[tokio::test]
    async fn creates_normalizes_and_serializes_metadata_updates() {
        let store = Arc::new(Store::default());
        let metadata = service(Arc::clone(&store));
        assert_eq!(metadata.read().await.unwrap().agents, Some(BTreeMap::new()));
        metadata.set_title("First prompt".into()).await.unwrap();
        let current = metadata.read().await.unwrap();
        assert_eq!(current.title.as_deref(), Some("First prompt"));
        assert_eq!(current.is_custom_title, Some(true));
        metadata
            .register_agent("main".into(), AgentMeta::default())
            .await
            .unwrap();
        let writes = store.0.lock().unwrap().len();
        metadata
            .register_agent("main".into(), AgentMeta::default())
            .await
            .unwrap();
        assert_eq!(store.0.lock().unwrap().len(), writes);
    }
    #[test]
    fn normalizes_legacy_version_and_dates() {
        let raw: SessionMeta = serde_json::from_value(serde_json::json!({"id":"old","createdAt":0,"updatedAt":0,"archived":false,"workDir":"/legacy"})).unwrap();
        let normalized = normalize_session_meta(raw, "new");
        assert_eq!(normalized.id, "new");
        assert_eq!(normalized.version, Some(2));
        assert_eq!(normalized.cwd.as_deref(), Some("/legacy"));
    }
}
