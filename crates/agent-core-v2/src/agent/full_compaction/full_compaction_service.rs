//! Idle full-compaction placeholder service.

use std::sync::Arc;

use crate::_base::{
    di::{
        descriptors::SyncDescriptor,
        scope::{InstantiationType, LifecycleScope, register_scoped_service},
    },
    event::Event,
};

use super::{
    AGENT_FULL_COMPACTION_SERVICE_ID, AgentFullCompactionHooks, AgentFullCompactionServiceContract,
    AgentFullCompactionServiceHandle, FullCompactionInput, FullCompactionTask,
};

pub struct AgentFullCompactionService {
    hooks: AgentFullCompactionHooks,
}

impl Default for AgentFullCompactionService {
    fn default() -> Self {
        Self {
            hooks: AgentFullCompactionHooks::default(),
        }
    }
}

impl AgentFullCompactionServiceContract for AgentFullCompactionService {
    fn compacting(&self) -> Option<FullCompactionTask> {
        None
    }
    fn begin(&self, _: FullCompactionInput) -> bool {
        false
    }
    fn hooks(&self) -> &AgentFullCompactionHooks {
        &self.hooks
    }
    fn on_did_finish_compaction(&self) -> Event<FullCompactionTask> {
        Event::none()
    }
}

pub fn register_agent_full_compaction_service() {
    register_scoped_service(
        LifecycleScope::Agent,
        AGENT_FULL_COMPACTION_SERVICE_ID,
        SyncDescriptor::new(|_| {
            let service: Arc<dyn AgentFullCompactionServiceContract> =
                Arc::new(AgentFullCompactionService::default());
            Ok(AgentFullCompactionServiceHandle(service))
        }),
        InstantiationType::Delayed,
        "fullCompaction",
    );
}
