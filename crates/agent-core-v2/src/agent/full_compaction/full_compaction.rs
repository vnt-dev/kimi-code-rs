//! Full-compaction contract.
//!
//! Original: `agent/fullCompaction/fullCompaction.ts`.

use std::{ops::Deref, sync::Arc};

use crate::{
    _base::{di::instantiation::ServiceIdentifier, event::Event},
    hooks::OrderedHookSlot,
};

use super::CompactionSource;

#[derive(Clone, Debug)]
pub struct FullCompactionInput {
    pub source: CompactionSource,
    pub instruction: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FullCompactionTask {
    pub trigger: CompactionSource,
    pub token_count: u64,
    pub trace_id: Option<String>,
}

pub struct AgentFullCompactionHooks {
    pub on_will_compact: OrderedHookSlot<FullCompactionTask>,
}

impl Default for AgentFullCompactionHooks {
    fn default() -> Self {
        Self {
            on_will_compact: OrderedHookSlot::new(),
        }
    }
}

pub trait AgentFullCompactionServiceContract: Send + Sync {
    fn compacting(&self) -> Option<FullCompactionTask>;
    fn begin(&self, input: FullCompactionInput) -> bool;
    fn hooks(&self) -> &AgentFullCompactionHooks;
    fn on_did_finish_compaction(&self) -> Event<FullCompactionTask>;
}

#[derive(Clone)]
pub struct AgentFullCompactionServiceHandle(pub Arc<dyn AgentFullCompactionServiceContract>);

impl Deref for AgentFullCompactionServiceHandle {
    type Target = dyn AgentFullCompactionServiceContract;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub const AGENT_FULL_COMPACTION_SERVICE_ID: ServiceIdentifier<AgentFullCompactionServiceHandle> =
    ServiceIdentifier::new("agentFullCompactionService");
