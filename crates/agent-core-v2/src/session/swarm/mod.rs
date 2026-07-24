//! Session-scoped swarm batch contract.
//!
//! Original: `session/swarm/sessionSwarm.ts`.

pub mod agent_run_batch;
pub use agent_run_batch::*;

use std::{future::Future, ops::Deref, pin::Pin, sync::Arc, time::Duration};

use serde_json::Value;

use crate::{
    _base::{di::instantiation::ServiceIdentifier, utils::abort::AbortSignal},
    kosong::contract::usage::TokenUsage,
};

#[derive(Clone)]
pub struct SessionSwarmTaskBase<T> {
    pub data: T,
    pub profile_name: String,
    pub parent_tool_call_id: String,
    pub parent_tool_call_uuid: Option<String>,
    pub prompt: String,
    pub description: String,
    pub swarm_index: Option<u64>,
    pub swarm_item: Option<String>,
    pub run_in_background: bool,
    pub timeout: Option<Duration>,
    pub signal: Option<AbortSignal>,
}

#[derive(Clone)]
pub enum SessionSwarmTask<T> {
    Spawn(SessionSwarmTaskBase<T>),
    Resume {
        base: SessionSwarmTaskBase<T>,
        resume_agent_id: String,
    },
}

impl<T> SessionSwarmTask<T> {
    pub fn base(&self) -> &SessionSwarmTaskBase<T> {
        match self {
            Self::Spawn(base) | Self::Resume { base, .. } => base,
        }
    }
}

#[derive(Clone)]
pub struct SessionSwarmRunArgs<T> {
    pub caller_agent_id: String,
    pub tasks: Vec<SessionSwarmTask<T>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSwarmRunStatus {
    Completed,
    Failed,
    Aborted,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSwarmRunState {
    Started,
    NotStarted,
}

#[derive(Clone)]
pub struct SessionSwarmRunResult<T> {
    pub task: SessionSwarmTask<T>,
    pub agent_id: Option<String>,
    pub status: SessionSwarmRunStatus,
    pub state: Option<SessionSwarmRunState>,
    pub result: Option<String>,
    pub usage: Option<TokenUsage>,
    pub error: Option<String>,
}

pub type SessionSwarmFuture =
    Pin<Box<dyn Future<Output = Vec<SessionSwarmRunResult<Value>>> + Send>>;
pub trait SessionSwarmServiceContract: Send + Sync {
    fn get_swarm_item(
        &self,
        caller_agent_id: &str,
        agent_id: &str,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    fn run(&self, args: SessionSwarmRunArgs<Value>) -> SessionSwarmFuture;
    fn cancel(&self, caller_agent_id: &str);
}

#[derive(Clone)]
pub struct SessionSwarmServiceHandle(pub Arc<dyn SessionSwarmServiceContract>);
impl Deref for SessionSwarmServiceHandle {
    type Target = dyn SessionSwarmServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}
pub const SESSION_SWARM_SERVICE_ID: ServiceIdentifier<SessionSwarmServiceHandle> =
    ServiceIdentifier::new("sessionSwarmService");
