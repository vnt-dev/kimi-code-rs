//! Persisted session tool-policy service.
//!
//! Original: `packages/agent-core-v2/src/session/sessionToolPolicy/sessionToolPolicyService.ts`.

use std::{
    collections::HashSet,
    io,
};
use std::sync::{Arc};
use parking_lot::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::{
    _base::{
        di::{
            descriptors::SyncDescriptor,
            instantiation::ServicesAccessorExt,
            scope::{InstantiationType, LifecycleScope, register_scoped_service},
        },
        event::{AsyncEmitter, Event},
    },
    persistence::interface::atomic_document_store::{
        ATOMIC_DOCUMENT_STORE_SERVICE_ID, AtomicDocumentStoreHandle,
    },
    session::session_context::{SESSION_CONTEXT_ID, SessionContext},
};

use super::{
    SESSION_TOOL_POLICY_ID, SessionToolPolicyChangedEvent, SessionToolPolicyContract,
    SessionToolPolicyError, SessionToolPolicyHandle,
};

const STATE_KEY: &str = "state.json";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionToolPolicyState {
    disabled_tools: Vec<String>,
}

#[derive(Clone, Debug)]
enum ReadyState {
    Pending,
    Ready,
    Failed(String),
}

pub struct SessionToolPolicyService {
    store: AtomicDocumentStoreHandle,
    scope: String,
    change_emitter: AsyncEmitter<()>,
    state: Mutex<SessionToolPolicyState>,
    ready_state: AsyncMutex<ReadyState>,
    update_lock: AsyncMutex<()>,
}

impl SessionToolPolicyService {
    pub fn new(context: &SessionContext, store: AtomicDocumentStoreHandle) -> Self {
        Self {
            store,
            scope: context.scope(Some("tool-policy")),
            change_emitter: AsyncEmitter::new(),
            state: Mutex::new(SessionToolPolicyState::default()),
            ready_state: AsyncMutex::new(ReadyState::Pending),
            update_lock: AsyncMutex::new(()),
        }
    }

    async fn ensure_ready(&self) -> Result<(), SessionToolPolicyError> {
        let mut ready = self.ready_state.lock().await;
        match &*ready {
            ReadyState::Ready => return Ok(()),
            ReadyState::Failed(message) => return Err(boxed_error(message.clone())),
            ReadyState::Pending => {}
        }
        let result = self.load().await.map_err(|error| error.to_string());
        *ready = match result {
            Ok(()) => ReadyState::Ready,
            Err(message) => ReadyState::Failed(message),
        };
        match &*ready {
            ReadyState::Ready => Ok(()),
            ReadyState::Failed(message) => Err(boxed_error(message.clone())),
            ReadyState::Pending => unreachable!("ready state is set before returning"),
        }
    }

    // Original: load(). A malformed stored value is reported through the
    // document store; duplicate names are collapsed while retaining first
    // occurrence order, exactly like new Set(array).
    async fn load(&self) -> Result<(), SessionToolPolicyError> {
        let stored = self
            .store
            .get::<SessionToolPolicyState>(&self.scope, STATE_KEY)
            .await
            .map_err(|error| Box::new(error) as SessionToolPolicyError)?;
        if let Some(stored) = stored {
            self.state.lock().disabled_tools = dedupe_names(stored.disabled_tools);
        }
        Ok(())
    }

    async fn replace(&self, names: Vec<String>) -> Result<(), SessionToolPolicyError> {
        self.ensure_ready().await?;
        let disabled_tools = dedupe_names(names);
        if self.state.lock().disabled_tools == disabled_tools {
            return Ok(());
        }
        let next_state = SessionToolPolicyState { disabled_tools };
        self.store
            .set(&self.scope, STATE_KEY, &next_state)
            .await
            .map_err(|error| Box::new(error) as SessionToolPolicyError)?;
        *self.state.lock() = next_state;
        self.change_emitter
            .fire_async((), CancellationToken::new())
            .await;
        Ok(())
    }
}

#[async_trait]
impl SessionToolPolicyContract for SessionToolPolicyService {
    async fn ready(&self) -> Result<(), SessionToolPolicyError> {
        self.ensure_ready().await
    }

    fn on_did_change(&self) -> Event<SessionToolPolicyChangedEvent> {
        self.change_emitter.event()
    }

    fn disabled_tools(&self) -> Vec<String> {
        self.state.lock().disabled_tools.clone()
    }

    async fn set_disabled_tools(&self, names: Vec<String>) -> Result<(), SessionToolPolicyError> {
        let _update = self.update_lock.lock().await;
        self.replace(names).await
    }
}

pub fn register_session_tool_policy() {
    register_scoped_service(
        LifecycleScope::Session,
        SESSION_TOOL_POLICY_ID,
        SyncDescriptor::new(|accessor| {
            let context = accessor.get(SESSION_CONTEXT_ID)?;
            let store = accessor.get(ATOMIC_DOCUMENT_STORE_SERVICE_ID)?;
            let service: Arc<dyn SessionToolPolicyContract> =
                Arc::new(SessionToolPolicyService::new(&context, (*store).clone()));
            Ok(SessionToolPolicyHandle(service))
        }),
        InstantiationType::Eager,
        "sessionToolPolicy",
    );
}

fn dedupe_names(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

fn boxed_error(message: String) -> SessionToolPolicyError {
    Box::new(io::Error::other(message))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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
    struct Store {
        values: Mutex<HashMap<(String, String), Value>>,
    }

    #[async_trait]
    impl AtomicDocumentStoreService for Store {
        async fn get_value(&self, scope: &str, key: &str) -> Result<Option<Value>, StorageError> {
            Ok(self
                .values
                .lock()
                .get(&(scope.into(), key.into()))
                .cloned())
        }
        async fn set_value(
            &self,
            scope: &str,
            key: &str,
            value: Value,
        ) -> Result<(), StorageError> {
            self.values
                .lock()
                .insert((scope.into(), key.into()), value);
            Ok(())
        }
        async fn delete(&self, scope: &str, key: &str) -> Result<(), StorageError> {
            self.values
                .lock()
                .remove(&(scope.into(), key.into()));
            Ok(())
        }
        async fn list(
            &self,
            _scope: &str,
            _prefix: Option<&str>,
        ) -> Result<Vec<String>, StorageError> {
            Ok(vec![])
        }
        fn watch(&self, _scope: &str, _key: &str) -> Event<()> {
            Event::none()
        }
        fn acquire(&self, _scope: &str, _key: &str) -> DisposableHandle {
            disposable_none()
        }
    }

    fn service(store: Arc<Store>) -> SessionToolPolicyService {
        let context = make_session_context(SessionContextInput {
            session_id: "s".into(),
            workspace_id: "w".into(),
            session_dir: "/s".into(),
            session_scope: "sessions/w/s".into(),
            cwd: "/repo".into(),
            meta_scope: None,
        });
        SessionToolPolicyService::new(&context, AtomicDocumentStoreHandle(store))
    }

    #[tokio::test]
    async fn loads_dedupes_and_persists_only_effective_replacements() {
        let store = Arc::new(Store::default());
        let policy = service(Arc::clone(&store));
        policy
            .set_disabled_tools(vec!["Read".into(), "Read".into(), "Write".into()])
            .await
            .unwrap();
        assert_eq!(policy.disabled_tools(), vec!["Read", "Write"]);
        let state = store
            .values
            .lock()
            .values()
            .next()
            .cloned()
            .unwrap();
        assert_eq!(state["disabledTools"], serde_json::json!(["Read", "Write"]));
        policy
            .set_disabled_tools(vec!["Read".into(), "Write".into()])
            .await
            .unwrap();
        assert_eq!(store.values.lock().len(), 1);
    }
}
