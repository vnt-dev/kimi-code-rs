//! Flat agent registry and explicit-id single-flight creation.
//!
//! Original: `session/agentLifecycle/agentLifecycleService.ts`, registry
//! portions of `create()`, `get()`, `list()`, and `remove()`.

use parking_lot::Mutex;
use std::sync::Arc;
use std::{collections::HashMap, fmt};

use futures_util::{
    FutureExt,
    future::{BoxFuture, Shared},
};

use crate::{
    _base::{
        di::scope::ScopeHandle,
        event::{Emitter, Event},
    },
    agent::permission_policy::PermissionMode,
};

use super::{AgentListFilter, CreateAgentOptions, ForkAgentOptions};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLifecycleRegistryError(Arc<String>);

impl AgentLifecycleRegistryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(Arc::new(message.into()))
    }
}

impl fmt::Display for AgentLifecycleRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AgentLifecycleRegistryError {}

pub trait AgentScopeBootstrap: Send + Sync {
    fn create(
        &self,
        agent_id: String,
        options: CreateAgentOptions,
    ) -> BoxFuture<'static, Result<ScopeHandle, AgentLifecycleRegistryError>>;
    fn fork(
        &self,
        source: ScopeHandle,
        source_agent_id: String,
        options: ForkAgentOptions,
    ) -> BoxFuture<'static, Result<ScopeHandle, AgentLifecycleRegistryError>>;
    fn broadcast_permission_mode(&self, handle: &ScopeHandle, mode: PermissionMode);
    fn remove(
        &self,
        handle: ScopeHandle,
    ) -> BoxFuture<'static, Result<(), AgentLifecycleRegistryError>>;
}

type Creation = Shared<BoxFuture<'static, Result<ScopeHandle, AgentLifecycleRegistryError>>>;

/// The source registry is flat: parent-child relationship data belongs to the
/// caller domain, never to this map. Concrete scope bootstrap work is supplied
/// by `AgentScopeBootstrap` so this type can preserve registry ordering without
/// coupling it to wire/profile implementation details.
pub struct AgentLifecycleRegistry<B> {
    bootstrap: Arc<B>,
    handles: Mutex<HashMap<String, ScopeHandle>>,
    creating: Mutex<HashMap<String, Creation>>,
    next_agent_id: Mutex<u64>,
    did_create: Emitter<ScopeHandle>,
    did_dispose: Emitter<String>,
}

impl<B: AgentScopeBootstrap + 'static> AgentLifecycleRegistry<B> {
    pub fn new(bootstrap: Arc<B>) -> Self {
        Self {
            bootstrap,
            handles: Mutex::new(HashMap::new()),
            creating: Mutex::new(HashMap::new()),
            next_agent_id: Mutex::new(0),
            did_create: Emitter::new(),
            did_dispose: Emitter::new(),
        }
    }

    pub fn on_did_create(&self) -> Event<ScopeHandle> {
        self.did_create.event()
    }

    pub fn on_did_dispose(&self) -> Event<String> {
        self.did_dispose.event()
    }

    pub async fn create(
        &self,
        mut options: CreateAgentOptions,
    ) -> Result<ScopeHandle, AgentLifecycleRegistryError> {
        let explicit_id = options.agent_id.clone();
        let agent_id = explicit_id.clone().unwrap_or_else(|| self.next_agent_id());
        if options.agent_id.is_none() {
            options.agent_id = Some(agent_id.clone());
        }
        if explicit_id.is_none() {
            let handle = self.bootstrap.create(agent_id.clone(), options).await?;
            self.handles.lock().insert(agent_id, handle.clone());
            self.did_create.fire(&handle);
            return Ok(handle);
        }
        if let Some(handle) = self.get(&agent_id) {
            return Ok(handle);
        }
        let (creation, owner) = {
            let mut creating = self.creating.lock();
            if let Some(creation) = creating.get(&agent_id) {
                (creation.clone(), false)
            } else {
                let bootstrap = Arc::clone(&self.bootstrap);
                let id = agent_id.clone();
                let creation = async move { bootstrap.create(id, options).await }
                    .boxed()
                    .shared();
                creating.insert(agent_id.clone(), creation.clone());
                (creation, true)
            }
        };
        let result = creation.await;
        if owner {
            self.creating.lock().remove(&agent_id);
            if let Ok(handle) = &result {
                self.handles.lock().insert(agent_id, handle.clone());
                self.did_create.fire(handle);
            }
        }
        result
    }

    pub async fn fork(
        &self,
        source_agent_id: &str,
        options: ForkAgentOptions,
    ) -> Result<ScopeHandle, AgentLifecycleRegistryError> {
        let source = self.get(source_agent_id).ok_or_else(|| {
            AgentLifecycleRegistryError::new(format!(
                "Source agent \"{source_agent_id}\" does not exist"
            ))
        })?;
        if let Some(agent_id) = &options.agent_id
            && self.get(agent_id).is_some()
        {
            return Err(AgentLifecycleRegistryError::new(format!(
                "Agent \"{agent_id}\" already exists"
            )));
        }
        let handle = self
            .bootstrap
            .fork(source, source_agent_id.to_owned(), options)
            .await?;
        let id = handle.id().to_owned();
        self.handles.lock().insert(id, handle.clone());
        self.did_create.fire(&handle);
        Ok(handle)
    }

    pub fn get(&self, agent_id: &str) -> Option<ScopeHandle> {
        self.handles.lock().get(agent_id).cloned()
    }

    pub fn list(&self, filter: Option<&AgentListFilter>) -> Vec<ScopeHandle> {
        let handles = self.handles.lock();
        handles
            .iter()
            .filter(|(id, _)| {
                filter
                    .and_then(|filter| filter.prefix.as_deref())
                    .is_none_or(|prefix| id.starts_with(prefix))
            })
            .map(|(_, handle)| handle.clone())
            .collect()
    }

    pub fn broadcast_permission_mode(&self, mode: PermissionMode) {
        for handle in self.list(None) {
            self.bootstrap.broadcast_permission_mode(&handle, mode);
        }
    }

    pub async fn remove(&self, agent_id: &str) -> Result<(), AgentLifecycleRegistryError> {
        let Some(handle) = self.handles.lock().remove(agent_id) else {
            return Ok(());
        };
        self.bootstrap.remove(handle).await?;
        self.did_dispose.fire(&agent_id.to_owned());
        Ok(())
    }

    fn next_agent_id(&self) -> String {
        let mut next = self.next_agent_id.lock();
        let id = format!("agent-{}", *next);
        *next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::_base::di::{
        lifecycle::Disposable,
        scope::{LifecycleScope, Scope, ScopeOptions},
    };

    use super::*;

    struct ScopeBootstrap {
        session: Scope,
        creates: AtomicUsize,
        broadcasts: AtomicUsize,
    }

    impl ScopeBootstrap {
        fn new() -> Self {
            let app = Scope::create_app(ScopeOptions {
                id: Some("app".into()),
                ..ScopeOptions::default()
            });
            let session = app
                .create_child(LifecycleScope::Session, "session", ScopeOptions::default())
                .unwrap();
            Self {
                session,
                creates: AtomicUsize::new(0),
                broadcasts: AtomicUsize::new(0),
            }
        }
    }

    impl AgentScopeBootstrap for ScopeBootstrap {
        fn create(
            &self,
            agent_id: String,
            _options: CreateAgentOptions,
        ) -> BoxFuture<'static, Result<ScopeHandle, AgentLifecycleRegistryError>> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            let session = self.session.clone();
            Box::pin(async move {
                tokio::task::yield_now().await;
                session
                    .create_child(LifecycleScope::Agent, agent_id, ScopeOptions::default())
                    .map(|scope| scope.to_handle())
                    .map_err(|error| AgentLifecycleRegistryError::new(error.to_string()))
            })
        }

        fn fork(
            &self,
            _source: ScopeHandle,
            _source_agent_id: String,
            options: ForkAgentOptions,
        ) -> BoxFuture<'static, Result<ScopeHandle, AgentLifecycleRegistryError>> {
            let id = options.agent_id.unwrap_or_else(|| "fork".into());
            let session = self.session.clone();
            Box::pin(async move {
                session
                    .create_child(LifecycleScope::Agent, id, ScopeOptions::default())
                    .map(|scope| scope.to_handle())
                    .map_err(|error| AgentLifecycleRegistryError::new(error.to_string()))
            })
        }

        fn broadcast_permission_mode(&self, _handle: &ScopeHandle, _mode: PermissionMode) {
            self.broadcasts.fetch_add(1, Ordering::SeqCst);
        }

        fn remove(
            &self,
            handle: ScopeHandle,
        ) -> BoxFuture<'static, Result<(), AgentLifecycleRegistryError>> {
            Box::pin(async move {
                handle
                    .dispose()
                    .map_err(|error| AgentLifecycleRegistryError::new(error.to_string()))
            })
        }
    }

    #[tokio::test]
    async fn explicit_ids_join_creation_and_registry_stays_flat() {
        let bootstrap = Arc::new(ScopeBootstrap::new());
        let registry = Arc::new(AgentLifecycleRegistry::new(Arc::clone(&bootstrap)));
        let first = registry.create(CreateAgentOptions {
            agent_id: Some("worker".into()),
            ..Default::default()
        });
        let second = registry.create(CreateAgentOptions {
            agent_id: Some("worker".into()),
            ..Default::default()
        });
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().id(), "worker");
        assert_eq!(second.unwrap().id(), "worker");
        assert_eq!(bootstrap.creates.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .list(Some(&AgentListFilter {
                    prefix: Some("work".into())
                }))
                .len(),
            1
        );

        registry.broadcast_permission_mode(PermissionMode::Manual);
        assert_eq!(bootstrap.broadcasts.load(Ordering::SeqCst), 1);
        registry.remove("worker").await.unwrap();
        assert!(registry.get("worker").is_none());
    }
}
