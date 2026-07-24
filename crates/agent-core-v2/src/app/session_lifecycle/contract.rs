//! Process-wide session lifecycle contract.
//!
//! Original: `packages/agent-core-v2/src/app/sessionLifecycle/sessionLifecycle.ts`.

use std::{collections::HashMap, error::Error, ops::Deref, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    _base::{
        di::{instantiation::ServiceIdentifier, scope::ScopeHandle},
        event::Event,
    },
    agent::{mcp::McpServerConfig, profile::BindAgentInput},
    hooks::OrderedHookSlot,
};

pub type SessionScopeHandle = ScopeHandle;
pub type SessionLifecycleError = Box<dyn Error + Send + Sync>;

#[derive(Clone, Default)]
pub struct CreateSessionOptions {
    pub session_id: Option<String>,
    pub work_dir: String,
    pub additional_dirs: Option<Vec<String>>,
    pub mcp_servers: Option<HashMap<String, McpServerConfig>>,
    pub main_agent_binding: Option<BindAgentInput>,
}

#[derive(Clone, Default)]
pub struct ForkSessionOptions {
    pub source_session_id: String,
    pub new_session_id: Option<String>,
    pub title: Option<String>,
    pub metadata: Option<serde_json::Map<String, Value>>,
}

pub type CreateChildSessionOptions = ForkSessionOptions;

#[derive(Clone)]
pub struct SessionCreatedEvent {
    pub session_id: String,
    pub handle: SessionScopeHandle,
    pub source: SessionCreateSource,
}

#[derive(Clone)]
pub struct SessionClosedEvent {
    pub session_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCreateSource {
    Startup,
    Resume,
    Fork,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCloseReason {
    Exit,
}

#[derive(Clone)]
pub struct SessionWillCloseEvent {
    pub session_id: String,
    pub handle: SessionScopeHandle,
    pub reason: SessionCloseReason,
}

pub struct SessionLifecycleHooks {
    pub on_did_create_session: OrderedHookSlot<SessionCreatedEvent>,
    pub on_will_close_session: OrderedHookSlot<SessionWillCloseEvent>,
}

impl Default for SessionLifecycleHooks {
    fn default() -> Self {
        Self {
            on_did_create_session: OrderedHookSlot::new(),
            on_will_close_session: OrderedHookSlot::new(),
        }
    }
}

#[derive(Clone)]
pub struct SessionArchivedEvent {
    pub session_id: String,
}

#[derive(Clone)]
pub struct SessionForkedEvent {
    pub source_session_id: String,
    pub session_id: String,
    pub handle: SessionScopeHandle,
}

#[async_trait]
pub trait SessionLifecycleServiceContract: Send + Sync {
    fn on_did_create_session(&self) -> Event<SessionCreatedEvent>;
    fn on_did_close_session(&self) -> Event<SessionClosedEvent>;
    fn on_did_archive_session(&self) -> Event<SessionArchivedEvent>;
    fn on_did_fork_session(&self) -> Event<SessionForkedEvent>;
    fn hooks(&self) -> &SessionLifecycleHooks;
    async fn create(
        &self,
        options: CreateSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError>;
    fn get(&self, session_id: &str) -> Option<SessionScopeHandle>;
    fn list(&self) -> Vec<SessionScopeHandle>;
    async fn resume(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError>;
    async fn close(&self, session_id: &str) -> Result<(), SessionLifecycleError>;
    async fn archive(&self, session_id: &str) -> Result<(), SessionLifecycleError>;
    async fn restore(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionScopeHandle>, SessionLifecycleError>;
    async fn fork(
        &self,
        options: ForkSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError>;
    async fn create_child(
        &self,
        options: CreateChildSessionOptions,
    ) -> Result<SessionScopeHandle, SessionLifecycleError>;
}

#[derive(Clone)]
pub struct SessionLifecycleServiceHandle(pub Arc<dyn SessionLifecycleServiceContract>);

impl Deref for SessionLifecycleServiceHandle {
    type Target = dyn SessionLifecycleServiceContract;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const SESSION_LIFECYCLE_SERVICE_ID: ServiceIdentifier<SessionLifecycleServiceHandle> =
    ServiceIdentifier::new("sessionLifecycleService");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_identifiers_and_sources_match_source_contract() {
        assert_eq!(
            SESSION_LIFECYCLE_SERVICE_ID.to_string(),
            "sessionLifecycleService"
        );
        assert_eq!(SessionCreateSource::Startup, SessionCreateSource::Startup);
        assert_ne!(SessionCreateSource::Resume, SessionCreateSource::Fork);
        assert_eq!(SessionCloseReason::Exit, SessionCloseReason::Exit);
        let hooks = SessionLifecycleHooks::default();
        let disposable = hooks.on_did_create_session.as_disposable("test");
        drop(disposable);
    }
}
